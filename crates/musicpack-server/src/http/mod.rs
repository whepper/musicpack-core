//! Minimal blocking HTTP/1.1 server for the read-only API.
//!
//! The Phase 14 investigation recommended a hand-rolled `std`-only
//! implementation, and nothing in the read-only contract needs more: the
//! surface is ~10 JSON endpoints plus session exchange, all semantics are
//! fully specified by the legacy behaviour, and every alternative (async
//! runtime, framework) would add dependency risk for zero compatibility
//! gain. The server is synchronous and blocking — one thread per
//! connection, one shared store connection behind a lock (like the C
//! serving process, which funnels everything through a single SQLite
//! handle).
//!
//! Deliberate transport simplifications (all documented, none observable
//! through the JSON contract): `Connection: close` always (no keep-alive
//! negotiation), no chunked-request support, absolute-form targets reduced
//! to origin-form. Malformed requests close the connection without a
//! response (MHD rejects them at the transport layer the same way).

pub mod auth;
pub mod json;
pub mod range;
pub mod request;
pub mod response;
pub mod routes;
pub mod serve;
pub mod staticfiles;

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use request::{BODY_MAX, HEAD_MAX, ParseError, Request, parse};
use response::classify_cors;
use routes::Context;

use crate::shutdown::Shutdown;
use crate::store::Store;

/// Maximum bytes read for one request (headers + the 4 KiB body cap).
const READ_MAX: usize = HEAD_MAX + 4 + BODY_MAX;
/// Read timeout per connection (the C idles out at 60 s; tests never wait
/// that long, but production behaviour matches).
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Upper bound on draining in-flight connections / a running job at
/// shutdown. After it, the process would be terminated by the supervisor.
pub const DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
/// How often the internal waker checks the shutdown token (the accept loop
/// itself blocks, so this is the only poll).
const WAKE_POLL: Duration = Duration::from_millis(25);

/// Serves on an already-bound listener until the process ends. Kept for the
/// existing call sites/tests; equivalent to [`serve_with_shutdown`] with a
/// token nobody requests.
pub fn serve_on<S: Store + Send + 'static>(
    listener: TcpListener,
    ctx: Arc<Context<S>>,
) -> std::io::Result<()> {
    serve_with_shutdown(listener, ctx, Shutdown::new())
}

/// Serves until `shutdown` is requested, then stops accepting, drains
/// in-flight connections (bounded by [`DRAIN_TIMEOUT`]) and returns.
///
/// The accept loop stays **blocking** (preserving the C's accept latency);
/// shutdown wakes it by connecting to the listener's own address from an
/// internal waker thread. No signals, no async runtime, no accept polling on
/// the hot path.
pub fn serve_with_shutdown<S: Store + Send + 'static>(
    listener: TcpListener,
    ctx: Arc<Context<S>>,
    shutdown: Shutdown,
) -> std::io::Result<()> {
    let wake_addr = listener.local_addr()?;
    {
        let shutdown = shutdown.clone();
        let _ = std::thread::Builder::new()
            .name("musicpack-shutdown-waker".into())
            .spawn(move || {
                while !shutdown.is_requested() {
                    std::thread::sleep(WAKE_POLL);
                }
                // The woken accept is discarded (the loop checks the token
                // before handling); any connect error is irrelevant.
                let _ = TcpStream::connect(wake_addr);
            });
    }
    loop {
        let (stream, _) = match listener.accept() {
            Ok(pair) => pair,
            Err(_) => continue,
        };
        if shutdown.is_requested() {
            break;
        }
        let ctx = Arc::clone(&ctx);
        let guard = shutdown.guard();
        let _ = std::thread::Builder::new()
            .name("musicpack-conn".into())
            .spawn(move || {
                let _guard = guard;
                handle_connection(stream, &ctx);
            });
    }
    crate::logging::info("shutdown requested; draining in-flight requests");
    let drained = shutdown.wait_idle(DRAIN_TIMEOUT);
    if drained {
        crate::logging::info("drain complete; all in-flight requests finished");
    } else {
        crate::logging::log(
            crate::logging::Level::Warn,
            &format!(
                "drain timed out after {}s; {} request(s) still in flight",
                DRAIN_TIMEOUT.as_secs(),
                shutdown.in_flight()
            ),
        );
    }
    Ok(())
}

/// Binds `listen:port` (IPv4, loopback by default — never an accidental
/// wildcard, like the C `make_listen_socket`) and serves.
pub fn serve<S: Store + Send + 'static>(
    ctx: Arc<Context<S>>,
    listen: &str,
    port: u16,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(format!("{listen}:{port}"))?;
    serve_on(listener, ctx)
}

/// Binds and serves with a shutdown token.
pub fn serve_with_token<S: Store + Send + 'static>(
    ctx: Arc<Context<S>>,
    listen: &str,
    port: u16,
    shutdown: Shutdown,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(format!("{listen}:{port}"))?;
    serve_with_shutdown(listener, ctx, shutdown)
}

fn handle_connection<S: Store>(mut stream: TcpStream, ctx: &Context<S>) {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => return,
        }
        if buf.len() > READ_MAX {
            return;
        }
        match parse(&buf) {
            Err(ParseError::Truncated) => continue,
            Err(_) => return,
            Ok(None) => continue,
            Ok(Some((request, _used))) => {
                respond(stream, ctx, &request);
                return;
            }
        }
    }
}

fn respond<S: Store>(mut stream: TcpStream, ctx: &Context<S>, request: &Request) {
    // Static hosting runs entirely before the API's CORS/method gates —
    // the C `access_handler` branch. Its header set (incl. COOP/COEP and
    // the bare 404) is the static handler's own contract.
    if staticfiles::handles(&ctx.config.static_dir, &request.path) {
        let mut response =
            staticfiles::serve(&ctx.config.static_dir, &request.path, &request.method);
        let _ = response.write_to(&mut stream, request.method == "HEAD");
        return;
    }
    let cors = classify_cors(&ctx.config, &request.headers, &request.method);
    // `Secure` cookies when forced or behind a TLS-terminating proxy (the
    // C `request_is_secure`).
    let forwarded_https = request
        .headers
        .get("x-forwarded-proto")
        .is_some_and(|v| v.eq_ignore_ascii_case("https"));
    let secure = ctx.config.secure_cookies || forwarded_https;
    let mut response = routes::dispatch(ctx, request, &cors, secure);
    let head_only = request.method == "HEAD";
    let _ = response.write_to(&mut stream, head_only);
}
