//! Argument parsing and command dispatch — the port of the reference's
//! `main.c` CLI surface.
//!
//! Conventions preserved from the reference:
//!
//! - the first argument is the command (`scan` | `serve` | `verify` |
//!   `token`); options come after it;
//! - `help`/`--help`/`-h` and `version`/`--version` work as commands *and*
//!   as options, and always exit 0;
//! - unknown commands and usage problems exit 2; runtime failures exit 1;
//! - configuration precedence: defaults → environment → flags, with the
//!   port validated (`invalid port`) after parsing;
//! - token output formats (create/list/revoke) are reproduced verbatim,
//!   including the reference's row/column spacing;
//! - `token create` ignores extra positionals (only `--name` carries a
//!   name), `token revoke <id>` takes the id positionally.

use std::process::ExitCode;

use crate::config::Config;
use crate::error::CliError;
use crate::store::{Store, sqlite::SqliteStore};

/// The parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Index the library (deferred to the scanner stage).
    Scan,
    /// Serve the HTTP API (deferred to the HTTP stage).
    Serve,
    /// Scan with full integrity verification (deferred).
    Verify,
    /// Token management (functional in stage 1).
    Token(TokenSubcommand),
    /// Print usage, exit 0.
    Help,
    /// Print the version, exit 0.
    Version,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenSubcommand {
    Create { name: Option<String> },
    List,
    Revoke { id: String },
}

/// The reference's usage text (`usage()` in `main.c`), printed to stderr.
pub fn usage() {
    eprint!(
        "usage: musicpack-server <command> [options]\n\
         commands:\n\
         \x20 scan     index the library (deterministic, idempotent)\n\
         \x20 serve    scan (unless --no-scan) then serve the HTTP API\n\
         \x20 verify   scan with full integrity verification (sha256)\n\
         \x20 token    manage API bearer tokens: create / list / revoke\n\
         \x20 help     show this message\n\
         \x20 version  show version\n\
         options:\n\
         \x20 --library DIR     music root (default ./library, env MUSICPACK_LIBRARY)\n\
         \x20 --database PATH   sqlite database (default ./library.db, env MUSICPACK_DATABASE)\n\
         \x20 --listen IP       bind address (default 127.0.0.1, env MUSICPACK_LISTEN)\n\
         \x20 --port N          listen port (default 8080, env MUSICPACK_PORT)\n\
         \x20 --verify          full sha256 integrity verification during scan\n\
         \x20 --no-scan         serve without a startup scan\n\
         \x20 --static-dir DIR  serve the reference demo/static files under DIR\n\
         \x20 --allow-origin URL  allow a CORS origin (repeatable; default none)\n\
         \x20 --secure-cookies  always send Secure on session cookies (HTTPS)\n\
         \x20 --shutdown-file PATH  drain and exit when PATH appears (env MUSICPACK_SHUTDOWN_FILE)\n\
         token options:\n\
         \x20 --name NAME       token display name (token create)\n"
    );
}

/// A `--name` value stashed through the positional channel (the reference
/// parks it in a file-scope global).
struct ParsedArgs {
    config: Config,
    positional: Vec<String>,
}

/// Parses `args` (everything after the command word) into configuration and
/// remaining positionals, mirroring the reference's `parse_options`.
fn parse_options(args: &[String]) -> Result<ParsedArgs, CliError> {
    let mut config = Config::defaults().apply_env();
    let mut positional = Vec::new();
    let mut i = 0usize;
    while i < args.len() {
        let arg = args[i].as_str();
        let next_value = |i: &mut usize| -> Result<String, CliError> {
            *i += 1;
            args.get(*i)
                .cloned()
                .ok_or_else(|| CliError::Usage(format!("option {arg} requires a value")))
        };
        match arg {
            "--library" => config.library = next_value(&mut i)?,
            "--database" => config.database = next_value(&mut i)?,
            "--listen" => config.listen = next_value(&mut i)?,
            "--port" => {
                let raw = next_value(&mut i)?;
                // The reference's atoi(): unparseable text becomes 0, which
                // the shared port validation rejects.
                config.port = raw.parse().unwrap_or(0);
            }
            "--verify" => config.verify_on_scan = true,
            "--no-scan" => config.no_scan = true,
            "--static-dir" => config.static_dir = next_value(&mut i)?,
            "--allow-origin" => {
                let origin = next_value(&mut i)?;
                if !config.add_origin(&origin) {
                    eprintln!(
                        "musicpack-server: too many --allow-origin values (max {})",
                        crate::config::ALLOW_ORIGIN_MAX
                    );
                }
            }
            "--secure-cookies" => config.secure_cookies = true,
            "--shutdown-file" => config.shutdown_file = next_value(&mut i)?,
            "--name" => {
                let value = next_value(&mut i)?;
                positional.push(format!("--name={value}"));
            }
            "--help" | "-h" => return Err(CliError::HelpRequested),
            "--version" => return Err(CliError::VersionRequested),
            other => {
                if other.starts_with('-') {
                    return Err(CliError::Usage(format!("unrecognized option '{other}'")));
                }
                positional.push(other.to_string());
            }
        }
        i += 1;
    }
    Ok(ParsedArgs { config, positional })
}

/// Parses the full argument list and returns the command with its final
/// configuration.
pub fn parse(args: &[String]) -> Result<(Command, Config), CliError> {
    let Some(command) = args.first() else {
        return Err(CliError::Usage("missing command".into()));
    };
    match command.as_str() {
        "help" | "--help" | "-h" => return Ok((Command::Help, Config::defaults())),
        "version" | "--version" => return Ok((Command::Version, Config::defaults())),
        "scan" | "serve" | "verify" | "token" => {}
        other => return Err(CliError::Usage(format!("unknown command '{other}'"))),
    }
    let ParsedArgs { config, positional } = match parse_options(&args[1..]) {
        Ok(parsed) => parsed,
        // `--help`/`--version` anywhere exit 0 like the reference's getopt
        // handlers; the half-parsed configuration is irrelevant.
        Err(CliError::HelpRequested) => return Ok((Command::Help, Config::defaults())),
        Err(CliError::VersionRequested) => return Ok((Command::Version, Config::defaults())),
        Err(other) => return Err(other),
    };
    if config.port == 0 {
        // The reference validates `port > 0 && port <= 65535` after parsing;
        // u16 parsing already rejects > 65535, so 0 (unset/garbage) is the
        // only reachable failure.
        return Err(CliError::Usage(format!("invalid port {}", config.port)));
    }
    let command = match command.as_str() {
        "scan" => Command::Scan,
        "serve" => Command::Serve,
        "verify" => Command::Verify,
        _ => Command::Token(parse_token(&positional)?),
    };
    Ok((command, config))
}

/// Parses the token subcommand from the remaining positionals (`run_token`'s
/// argument handling). `--name=VALUE` entries were stashed by
/// [`parse_options`].
fn parse_token(positional: &[String]) -> Result<TokenSubcommand, CliError> {
    let mut name = None;
    let mut words: Vec<&str> = Vec::new();
    for item in positional {
        if let Some(value) = item.strip_prefix("--name=") {
            name = Some(value.to_string());
        } else {
            words.push(item);
        }
    }
    let Some(sub) = words.first() else {
        return Err(CliError::Usage(
            "usage: musicpack-server token create|list|revoke".into(),
        ));
    };
    match *sub {
        "create" => Ok(TokenSubcommand::Create { name }),
        "list" => Ok(TokenSubcommand::List),
        "revoke" => match words.get(1) {
            Some(id) => Ok(TokenSubcommand::Revoke {
                id: (*id).to_string(),
            }),
            None => Err(CliError::Usage(
                "usage: musicpack-server token revoke <id>".into(),
            )),
        },
        other => Err(CliError::Usage(format!(
            "musicpack-server: unknown token subcommand '{other}'"
        ))),
    }
}

/// Prints the version banner (the reference's `version` command).
pub fn print_version() {
    println!("musicpack-server {}", env!("CARGO_PKG_VERSION"));
}

/// Runs `serve`: opens the database (creating + migrating it), runs the
/// startup scan unless `--no-scan` (which requires an existing database),
/// then serves the read-only HTTP API until the process ends.
pub fn run_serve(config: &Config) -> Result<ExitCode, CliError> {
    use crate::store::sqlite::SqliteStore;

    if config.no_scan && !std::path::Path::new(&config.database).is_file() {
        return Err(CliError::Failure(format!(
            "musicpack-server: database '{}' does not exist; run \
             `musicpack-server scan` first (or drop --no-scan)",
            config.database
        )));
    }
    let mut store = SqliteStore::open(std::path::Path::new(&config.database))
        .map_err(|e| CliError::Failure(format!("musicpack-server: cannot open database: {e}")))?;
    if !config.no_scan {
        crate::logging::info("startup scan");
        if let Err(e) = crate::ingest::scan(
            &mut store,
            std::path::Path::new(&config.library),
            config.verify_on_scan,
        ) {
            crate::logging::error(&format!(
                "startup scan failed ({e}); serving with previous library state"
            ));
        }
    }
    let shutdown = crate::shutdown::Shutdown::new();
    if !config.shutdown_file.is_empty() {
        arm_shutdown_file(&config.shutdown_file, shutdown.clone());
    }
    let jobs = crate::jobs::shared();
    let ctx = std::sync::Arc::new(crate::http::routes::Context {
        config: config.clone(),
        store: std::sync::Mutex::new(store),
        jobs: jobs.clone(),
        shutdown: shutdown.clone(),
    });
    crate::logging::info(&format!(
        "serving http://{}:{} (library={}, database={})",
        config.listen, config.port, config.library, config.database
    ));
    crate::http::serve_with_token(ctx, &config.listen, config.port, shutdown.clone())
        .map_err(|e| CliError::Failure(format!("musicpack-server: cannot serve: {e}")))?;
    // The accept loop drained its in-flight connections; a running job
    // observes the same token and stops between packages (committed prefix
    // retained, never a spurious unavailable sweep). Bounded like the drain.
    if !crate::jobs::wait_idle(&jobs, crate::http::DRAIN_TIMEOUT) {
        crate::logging::log(
            crate::logging::Level::Warn,
            "shutdown: background job did not finish within the drain window",
        );
    }
    crate::logging::info("shutdown complete");
    Ok(ExitCode::SUCCESS)
}

/// Arms the graceful-shutdown trigger file: when `path` appears, the server
/// drains and exits. A supervisor's `ExecStop`/pre-stop hook creates it (a
/// std-only alternative to signal handling — see ADR 0009/0014). The file is
/// removed once observed so a restart is not immediately stopped.
fn arm_shutdown_file(path: &str, shutdown: crate::shutdown::Shutdown) {
    let path = path.to_string();
    crate::logging::info(&format!("graceful shutdown armed on file '{path}'"));
    let watcher = std::thread::Builder::new()
        .name("musicpack-shutdown".into())
        .spawn(move || {
            loop {
                if std::path::Path::new(&path).exists() {
                    let _ = std::fs::remove_file(&path);
                    crate::logging::info("shutdown file observed; draining");
                    shutdown.request();
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        });
    if watcher.is_err() {
        crate::logging::error("shutdown: cannot start the shutdown-file watcher");
    }
}

/// Runs a library scan (`scan`) or a verifying scan (`verify`) end to
/// end against the configured database and prints the reference's summary
/// line (`scan: T packages (A added, U updated, M moved, R removed,
/// I invalid)`).
pub fn run_scan(config: &Config, verify: bool) -> Result<ExitCode, CliError> {
    use crate::store::sqlite::SqliteStore;

    let mut store = SqliteStore::open(std::path::Path::new(&config.database))
        .map_err(|e| CliError::Failure(format!("musicpack-server: cannot open database: {e}")))?;
    let result = crate::ingest::scan(&mut store, std::path::Path::new(&config.library), verify)
        .map_err(|e| CliError::Failure(format!("musicpack-server: scan failed: {e}")))?;
    println!(
        "scan: {} packages ({} added, {} updated, {} moved, {} removed, {} invalid)",
        result.total, result.added, result.updated, result.moved, result.removed, result.invalid
    );
    Ok(ExitCode::SUCCESS)
}

/// Runs a token subcommand end-to-end against the configured database.
///
/// The store is opened writable like the reference's `mp_library_open`:
/// migrations run, so token management on a fresh path creates a valid
/// database.
pub fn run_token(subcommand: &TokenSubcommand, config: &Config) -> Result<ExitCode, CliError> {
    let mut store = SqliteStore::open(std::path::Path::new(&config.database))
        .map_err(|e| CliError::Failure(format!("musicpack-server: cannot open database: {e}")))?;
    match subcommand {
        TokenSubcommand::Create { name } => {
            // The reference defaults the display name to "unnamed".
            let name = name.as_deref().unwrap_or("unnamed");
            let (id, secret) = create_token(&mut store, name)?;
            println!("Token created: {name} (id {id})\n");
            println!("{secret}\n");
            println!("This token will not be shown again.");
        }
        TokenSubcommand::List => {
            let tokens = store
                .list_tokens()
                .map_err(|e| CliError::Failure(format!("musicpack-server: {e}")))?;
            // The reference's header and row formats, verbatim (note: the
            // row format genuinely uses single spaces between the last
            // columns while the header uses two).
            println!(
                "{:<4}  {:<20}  {:<22}  {:<22}  status",
                "id", "name", "created", "last used"
            );
            for row in &tokens {
                let status = if row.is_active() { "active" } else { "revoked" };
                let last_used = row.last_used_at.as_deref().unwrap_or("(never)");
                println!(
                    "{:>4}  {:<20} {:<22} {:<22} {}",
                    row.id, row.name, row.created_at, last_used, status
                );
            }
            println!("{} token(s)", tokens.len());
        }
        TokenSubcommand::Revoke { id } => {
            let Ok(id) = id.parse::<i64>() else {
                return Err(CliError::Usage(
                    "usage: musicpack-server token revoke <id>".into(),
                ));
            };
            if id <= 0 {
                return Err(CliError::Usage(
                    "usage: musicpack-server token revoke <id>".into(),
                ));
            }
            let revoked = store
                .revoke_token(id)
                .map_err(|e| CliError::Failure(format!("musicpack-server: {e}")))?;
            if revoked {
                println!("Token {id} revoked.");
            } else {
                println!("Token {id} not found or already revoked.");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Creates a token row for `name`, generating the secret and storing only
/// its hash. Returns `(row id, plaintext secret shown once)`.
fn create_token(store: &mut dyn Store, name: &str) -> Result<(i64, String), CliError> {
    let invalid = |e: crate::error::ServerError| {
        CliError::Failure(format!("musicpack-server: cannot create token: {e}"))
    };
    crate::tokens::validate_name(name).map_err(invalid)?;
    let secret = crate::tokens::generate_secret().map_err(invalid)?;
    let token_hash = crate::tokens::hash_secret(&secret);
    let id = store.create_token(name, &token_hash).map_err(invalid)?;
    Ok((id, secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn help_and_version_are_top_level_commands() {
        let (cmd, _) = parse(&args(&["help"])).unwrap();
        assert_eq!(cmd, Command::Help);
        let (cmd, _) = parse(&args(&["--version"])).unwrap();
        assert_eq!(cmd, Command::Version);
    }

    #[test]
    fn help_and_version_work_as_options_and_exit_cleanly() {
        // The reference's getopt OPT_HELP/OPT_VERSION exit 0 from anywhere.
        let (cmd, _) = parse(&args(&["scan", "--help"])).unwrap();
        assert_eq!(cmd, Command::Help);
        let (cmd, _) = parse(&args(&["token", "--version"])).unwrap();
        assert_eq!(cmd, Command::Version);
    }

    #[test]
    fn unknown_commands_are_usage_errors() {
        let e = parse(&args(&["frobnicate"])).unwrap_err();
        assert!(matches!(e, CliError::Usage(ref m) if m.contains("frobnicate")));
        let e = parse(&args(&[])).unwrap_err();
        assert!(matches!(e, CliError::Usage(ref m) if m.contains("missing command")));
    }

    #[test]
    fn flags_override_defaults() {
        let (cmd, config) =
            parse(&args(&["scan", "--library", "/srv/m", "--port", "9000"])).unwrap();
        assert_eq!(cmd, Command::Scan);
        assert_eq!(config.library, "/srv/m");
        assert_eq!(config.port, 9000);
        assert_eq!(config.database, "./library.db");
        assert_eq!(config.listen, "127.0.0.1");
    }

    #[test]
    fn allow_origin_accumulates_and_is_capped() {
        let mut argv = args(&["serve"]);
        for i in 0..10 {
            argv.push("--allow-origin".into());
            argv.push(format!("https://h{i}.example"));
        }
        let (_, config) = parse(&argv).unwrap();
        assert_eq!(config.allow_origin.len(), crate::config::ALLOW_ORIGIN_MAX);
    }

    #[test]
    fn port_validation_rejects_zero_garbage_and_overflow() {
        for raw in ["0", "not-a-number", "70000"] {
            let e = parse(&args(&["serve", "--port", raw])).unwrap_err();
            assert!(
                matches!(e, CliError::Usage(ref m) if m.contains("invalid port")),
                "port {raw} must be rejected"
            );
        }
        let (_, config) = parse(&args(&["serve", "--port", "65535"])).unwrap();
        assert_eq!(config.port, 65535);
    }

    #[test]
    fn token_subcommands_parse_from_positionals() {
        let (cmd, _) = parse(&args(&["token", "create", "--name", "Web"])).unwrap();
        assert_eq!(
            cmd,
            Command::Token(TokenSubcommand::Create {
                name: Some("Web".into())
            })
        );
        let (cmd, _) = parse(&args(&["token", "list"])).unwrap();
        assert_eq!(cmd, Command::Token(TokenSubcommand::List));
        let (cmd, _) = parse(&args(&["token", "revoke", "3"])).unwrap();
        assert_eq!(
            cmd,
            Command::Token(TokenSubcommand::Revoke { id: "3".into() })
        );
    }

    #[test]
    fn token_defaults_and_usage_errors_match_the_reference() {
        let (cmd, _) = parse(&args(&["token", "create"])).unwrap();
        assert_eq!(
            cmd,
            Command::Token(TokenSubcommand::Create { name: None }),
            "missing --name falls back to the reference's 'unnamed'"
        );
        // Extra positionals are ignored by create (the reference reads only
        // --name), while revoke takes its id positionally.
        let (cmd, _) = parse(&args(&["token", "create", "extra"])).unwrap();
        assert_eq!(cmd, Command::Token(TokenSubcommand::Create { name: None }));
        let e = parse(&args(&["token"])).unwrap_err();
        assert!(matches!(e, CliError::Usage(ref m) if m.contains("create|list|revoke")));
        let e = parse(&args(&["token", "revoke"])).unwrap_err();
        assert!(matches!(e, CliError::Usage(ref m) if m.contains("revoke <id>")));
        let e = parse(&args(&["token", "explode"])).unwrap_err();
        assert!(matches!(e, CliError::Usage(ref m)
                if m.contains("unknown token subcommand 'explode'")));
    }

    #[test]
    fn boolean_flags_are_recognized() {
        let (_, config) = parse(&args(&["scan", "--verify"])).unwrap();
        assert!(config.verify_on_scan);
        let (_, config) = parse(&args(&["serve", "--no-scan", "--secure-cookies"])).unwrap();
        assert!(config.no_scan);
        assert!(config.secure_cookies);
    }

    #[test]
    fn missing_and_unknown_options_are_usage_errors() {
        let e = parse(&args(&["scan", "--library"])).unwrap_err();
        assert!(matches!(e, CliError::Usage(ref m) if m.contains("--library")));
        let e = parse(&args(&["serve", "--nonesuch"])).unwrap_err();
        assert!(matches!(e, CliError::Usage(ref m) if m.contains("--nonesuch")));
    }
}
