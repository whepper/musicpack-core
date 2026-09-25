//! HTTP byte serving of an indexed `.mpak` member.
//!
//! The whole point of this suite is the invariant that makes a container member
//! safe to serve: **the response contains bytes belonging to exactly that
//! member, and nothing else.** A member is a window inside a much larger file,
//! so every test here is written to catch a leak in either direction —
//! container framing before the member, or the next member (and then the rest
//! of the container) after it.
//!
//! The path exercised is the real one: a real container (the committed
//! `fixtures/reference/reference-small.mpak`, and one packed at runtime with
//! core's own writer around the real SV8 corpus) → the real server scan → the
//! real store → the real `media::open` → the real `serve_media` decision tree.
//! Bytes are read back out of the produced `Response`, not asserted on a mock.
//!
//! The committed fixture is the primary one, per the repository's fixture
//! policy. `MUSICPACK_MPAK_FIXTURE` additionally exercises a large real
//! container when one is available; it skips loudly otherwise.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use musicpack_core::format::checksum::sha256_hex;
use musicpack_core::format::mpak::{PackMember, PackSource, write_mpak};
use musicpack_server::http::response::Response;
use musicpack_server::http::serve::serve_media;
use musicpack_server::ingest::scan;
use musicpack_server::media;
use musicpack_server::source::PackageSource;
use musicpack_server::store::MediaRef;
use musicpack_server::store::Store;
use musicpack_server::store::sqlite::SqliteStore;

const REFERENCE_CONTAINER: &str = "fixtures/reference/reference-small.mpak";
const REFERENCE_MEMBER: &str = "audio/01.bin";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn temp_root(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("mpak-serve-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An in-memory `PackSource` over a set of members.
struct Members {
    manifest: String,
    table: Vec<PackMember>,
    content: Vec<(String, Vec<u8>)>,
}

impl Members {
    fn bytes(&self, path: &str) -> &[u8] {
        self.content
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, b)| b.as_slice())
            .unwrap_or(&[])
    }
}

impl PackSource for Members {
    fn manifest_bytes(&self) -> &[u8] {
        self.manifest.as_bytes()
    }
    fn members(&self) -> &[PackMember] {
        &self.table
    }
    fn member_size(&self, path: &str) -> Result<u64, musicpack_core::Error> {
        Ok(self.bytes(path).len() as u64)
    }
    fn read_member(&self, path: &str) -> Result<Box<dyn Read>, musicpack_core::Error> {
        Ok(Box::new(std::io::Cursor::new(self.bytes(path).to_vec())))
    }
}

fn real_audio() -> Vec<u8> {
    let path = repo_root().join("tests/fixtures/musepack/sine44-q5.mpc");
    std::fs::read(&path).unwrap_or_else(|e| panic!("fixture corpus missing: {e}"))
}

/// Packs a three-member album into `out`: two real SV8 tracks and artwork, so
/// the fixture has several *distinct* members at distinct offsets — which is
/// what catches an accidental "always the first member" bug.
fn pack_album(out: &Path) -> Vec<(String, Vec<u8>)> {
    let audio = real_audio();
    // A distinct second member, so two members cannot be confused for one.
    let mut second = audio.clone();
    for byte in second.iter_mut() {
        *byte = byte.wrapping_add(7);
    }
    let artwork: Vec<u8> = (0..2048u32).map(|i| (i % 251) as u8).collect();
    let content = vec![
        ("audio/01.mpc".to_string(), audio),
        ("audio/02.mpc".to_string(), second),
        ("artwork/front.jpg".to_string(), artwork),
    ];
    let table: Vec<PackMember> = content
        .iter()
        .map(|(path, bytes)| PackMember {
            path: path.clone(),
            sha256_hex: sha256_hex(bytes),
        })
        .collect();
    let manifest = format!(
        r#"{{"format":"musicpack","version":1,"album":{{"title":"Served Album","artists":[{{"name":"The Server"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"One","audio":{{"path":"audio/01.mpc","sha256":"{}"}}}},{{"track":2,"title":"Two","audio":{{"path":"audio/02.mpc","sha256":"{}"}}}}]}}],"artwork":[{{"role":"front","path":"artwork/front.jpg","sha256":"{}"}}]}}"#,
        table[0].sha256_hex, table[1].sha256_hex, table[2].sha256_hex,
    );
    let source = Members {
        manifest,
        table,
        content: content.clone(),
    };
    let mut bytes = Vec::new();
    write_mpak(&source, &mut bytes).expect("pack");
    std::fs::write(out, &bytes).expect("write");
    content
}

fn store() -> SqliteStore {
    SqliteStore::open(Path::new(":memory:")).expect("open an in-memory store")
}

/// Indexes `library` and returns the store plus the container path.
fn index(root: &Path) -> SqliteStore {
    let mut store = store();
    let result = scan(&mut store, root, true).expect("scan");
    assert_eq!(
        result.invalid, 0,
        "every source indexed cleanly: {result:?}"
    );
    store
}

/// The stored audio reference for the track at `position` in manifest order.
fn audio_ref(store: &SqliteStore, position: usize) -> MediaRef {
    let track = store
        .release_media(1)
        .expect("release media")
        .into_iter()
        .flat_map(|m| m.tracks)
        .nth(position)
        .expect("a track");
    store
        .resolve_track_audio(track.id)
        .expect("resolve")
        .expect("a servable audio object")
}

/// A `MediaRef` for a member that is *not* in the container's member table.
fn foreign_ref(store: &SqliteStore, relative: &str) -> MediaRef {
    let mut r = audio_ref(store, 0);
    r.relative_path = relative.into();
    r
}

fn headers(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn header(response: &Response, name: &str) -> Option<String> {
    response
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
}

/// The response body bytes, read through the same `Body` the connection layer
/// streams — so a test cannot pass by inspecting the wrong field.
fn body_bytes(response: &mut Response) -> Vec<u8> {
    let mut out = Vec::new();
    // `Body` is private to the crate; the public `content_length` plus the
    // file slice is what the wire carries, so re-stream it the same way
    // `Response::write_to` does.
    let len = response.content_length() as usize;
    if len == 0 {
        return out;
    }
    match &mut response.body {
        musicpack_server::http::response::Body::Bytes(bytes) => out.extend_from_slice(bytes),
        musicpack_server::http::response::Body::FileRange { file, offset, len } => {
            use std::io::{Seek, SeekFrom};
            file.seek(SeekFrom::Start(*offset)).expect("seek");
            let mut chunk = vec![0u8; *len as usize];
            file.read_exact(&mut chunk).expect("read the served slice");
            out.extend_from_slice(&chunk);
        }
    }
    out
}

/// Serves `media_ref` with the given request headers and returns the response.
fn serve(media_ref: &MediaRef, pairs: &[(&str, &str)]) -> Response {
    let resource = media::open(media_ref).expect("the object opens");
    serve_media(&headers(pairs), resource)
}

#[test]
fn a_full_member_response_is_exactly_the_member() {
    let root = temp_root("full");
    let container = root.join("album.mpak");
    let members = pack_album(&container);
    let store = index(&root);
    let r = audio_ref(&store, 0);
    let expected = &members[0].1;

    let mut response = serve(&r, &[]);
    assert_eq!(response.status, 200);
    assert_eq!(
        response.content_length() as usize,
        expected.len(),
        "the logical size is the member's length, never the container's"
    );
    assert_eq!(
        body_bytes(&mut response),
        *expected,
        "the body is the member and not the container"
    );
    assert!(
        response.content_length() < std::fs::metadata(&container).unwrap().len() as u64,
        "the container is strictly larger than what was served"
    );
}

#[test]
fn a_bounded_range_is_member_relative() {
    let root = temp_root("bounded");
    let container = root.join("album.mpak");
    let members = pack_album(&container);
    let store = index(&root);
    let r = audio_ref(&store, 0);
    let expected = &members[0].1;

    let mut response = serve(&r, &[("range", "bytes=1000-1999")]);
    assert_eq!(response.status, 206);
    assert_eq!(response.content_length(), 1000);
    assert_eq!(
        header(&response, "Content-Range").as_deref(),
        Some(format!("bytes 1000-1999/{}", expected.len()).as_str()),
        "Content-Range is member-relative and exposes no container offset"
    );
    assert_eq!(body_bytes(&mut response), expected[1000..2000]);
}

#[test]
fn an_open_ended_range_stops_at_the_member_boundary() {
    let root = temp_root("open-ended");
    let container = root.join("album.mpak");
    let members = pack_album(&container);
    let store = index(&root);
    let r = audio_ref(&store, 0);
    let expected = &members[0].1;
    let container_len = std::fs::metadata(&container).unwrap().len() as usize;

    let mut response = serve(&r, &[("range", "bytes=1000-")]);
    assert_eq!(response.status, 206);
    assert_eq!(
        response.content_length() as usize,
        expected.len() - 1000,
        "the range ends at the member, not at the file"
    );
    assert_eq!(
        header(&response, "Content-Range").as_deref(),
        Some(format!("bytes 1000-{}/{}", expected.len() - 1, expected.len()).as_str())
    );
    let body = body_bytes(&mut response);
    assert_eq!(body, expected[1000..]);
    assert!(
        body.len() < container_len,
        "no container bytes past the member were served"
    );
}

#[test]
fn the_last_member_byte_is_served_and_nothing_after_it() {
    // The strongest leak test: a member is followed, inside the same file, by
    // another member and then by the container's index and tail.
    let root = temp_root("tail");
    let container = root.join("album.mpak");
    let members = pack_album(&container);
    let store = index(&root);
    let r = audio_ref(&store, 0);
    let expected = &members[0].1;
    let last = expected.len() - 1;

    let mut response = serve(&r, &[("range", &format!("bytes={last}-"))]);
    assert_eq!(response.status, 206);
    assert_eq!(response.content_length(), 1);
    assert_eq!(
        header(&response, "Content-Range").as_deref(),
        Some(format!("bytes {last}-{last}/{}", expected.len()).as_str())
    );
    let body = body_bytes(&mut response);
    assert_eq!(body, vec![expected[last]], "the final member byte, exactly");

    // The byte physically after the member in the file is NOT the last member
    // byte — proving the read stopped at the member boundary.
    let whole = std::fs::read(&container).unwrap();
    let (offset, length) = PackageSource::container(&container)
        .member_extent("audio/01.mpc")
        .expect("the member extent");
    assert_ne!(
        whole[offset as usize + length as usize],
        expected[last],
        "the test would be vacuous if the member ended the file"
    );
}

#[test]
fn a_range_past_the_member_is_refused_and_leaks_nothing() {
    let root = temp_root("out-of-range");
    let container = root.join("album.mpak");
    let members = pack_album(&container);
    let store = index(&root);
    let r = audio_ref(&store, 0);
    let size = members[0].1.len() as u64;

    for range in [
        format!("bytes={size}-"),
        format!("bytes={}-{}", size + 5000, size + 6000),
        "bytes=xyz".to_string(),
        "bytes=5-2".to_string(),
    ] {
        let mut response = serve(&r, &[("range", &range)]);
        assert_eq!(response.status, 416, "{range}");
        assert_eq!(
            header(&response, "Content-Range").as_deref(),
            Some(format!("bytes */{size}").as_str()),
            "{range}: the 416 boundary is the member length"
        );
        assert!(body_bytes(&mut response).is_empty(), "{range} leaked bytes");
    }
}

#[test]
fn a_suffix_range_clamps_to_the_member() {
    let root = temp_root("suffix");
    let container = root.join("album.mpak");
    let members = pack_album(&container);
    let store = index(&root);
    let r = audio_ref(&store, 0);
    let expected = &members[0].1;

    let mut response = serve(&r, &[("range", "bytes=-500")]);
    assert_eq!(response.status, 206);
    assert_eq!(response.content_length(), 500);
    assert_eq!(body_bytes(&mut response), expected[expected.len() - 500..]);
}

#[test]
fn two_members_resolve_to_different_bytes() {
    // Guards the "first member" and container-level fallback bugs: the two
    // tracks share a file, and each must return only its own bytes.
    let root = temp_root("two-members");
    let container = root.join("album.mpak");
    let members = pack_album(&container);
    let store = index(&root);

    let first = audio_ref(&store, 0);
    let second = audio_ref(&store, 1);
    assert_eq!(first.package_path, second.package_path, "one container");
    assert_ne!(first.relative_path, second.relative_path);

    let (first_extent, second_extent) = {
        let source = PackageSource::container(&container);
        (
            source
                .member_extent(&first.relative_path)
                .expect("first extent"),
            source
                .member_extent(&second.relative_path)
                .expect("second extent"),
        )
    };
    assert_ne!(
        first_extent, second_extent,
        "the two members occupy different absolute ranges"
    );

    let mut a = serve(&first, &[]);
    let mut b = serve(&second, &[]);
    let a_bytes = body_bytes(&mut a);
    let b_bytes = body_bytes(&mut b);
    assert_eq!(a_bytes, members[0].1, "member one returns its own bytes");
    assert_eq!(b_bytes, members[1].1, "member two returns its own bytes");
    assert_ne!(a_bytes, b_bytes, "the members really do differ");

    // And a range inside the second member never reaches the first.
    let mut ranged = serve(&second, &[("range", "bytes=10-19")]);
    assert_eq!(body_bytes(&mut ranged), members[1].1[10..20]);
}

#[test]
fn a_member_not_in_the_container_serves_nothing() {
    let root = temp_root("foreign-member");
    let container = root.join("album.mpak");
    pack_album(&container);
    let store = index(&root);

    // A member path that is not in the member table — the stored locator must
    // not be joined onto anything.
    for bogus in [
        "audio/99.mpc",
        "../outside.mpc",
        "/etc/passwd",
        "audio/../../escape.mpc",
    ] {
        let r = foreign_ref(&store, bogus);
        let error = media::open(&r).expect_err("a member outside the table is refused");
        assert!(
            matches!(error, media::MediaError::Unavailable(_)),
            "{bogus} must be a clean refusal, got {error:?}"
        );
    }
}

#[test]
fn a_corrupt_container_serves_nothing() {
    // Byte serving must not become a way to hand out arbitrary file content
    // when the container no longer validates.
    let root = temp_root("corrupt");
    let container = root.join("album.mpak");
    pack_album(&container);
    let store = index(&root);
    let r = audio_ref(&store, 0);
    assert!(media::open(&r).is_ok(), "the intact container serves");

    // Corrupt the container in place: the member table no longer validates, so
    // resolution fails and nothing is served.
    let mut bytes = std::fs::read(&container).unwrap();
    for byte in bytes.iter_mut().skip(64) {
        *byte ^= 0xff;
    }
    std::fs::write(&container, &bytes).unwrap();

    let error = media::open(&r).expect_err("a corrupt container serves nothing");
    assert!(
        matches!(error, media::MediaError::Unavailable(_)),
        "clean refusal, got {error:?}"
    );
}

#[test]
fn a_container_backed_track_is_reported_with_its_own_size() {
    // The resource's logical size and base are the whole contract with the
    // byte layer; assert them directly.
    let root = temp_root("resource");
    let container = root.join("album.mpak");
    let members = pack_album(&container);
    let store = index(&root);
    let r = audio_ref(&store, 0);
    let (offset, length) = PackageSource::container(&container)
        .member_extent(&r.relative_path)
        .expect("extent");

    let resource = media::open(&r).expect("open");
    assert_eq!(
        resource.size, length,
        "the logical size is the member length"
    );
    assert_eq!(resource.base, offset, "the base is the member's offset");
    assert!(resource.size < length + 1);
    assert_eq!(resource.size as usize, members[0].1.len());
    assert!(
        resource.base + resource.size <= std::fs::metadata(&container).unwrap().len(),
        "the window lies inside the container file"
    );
}

#[test]
fn the_committed_reference_container_serves_its_member() {
    // The primary fixture: a real container produced by the reference packer.
    let root = temp_root("committed");
    let container = root.join("reference-small.mpak");
    std::fs::copy(repo_root().join(REFERENCE_CONTAINER), &container)
        .expect("copy the committed fixture");
    let store = index(&root);

    let r = audio_ref(&store, 0);
    assert_eq!(r.relative_path, REFERENCE_MEMBER);

    let source = PackageSource::container(&container);
    let expected = source
        .members()
        .into_iter()
        .find(|(p, _)| p == REFERENCE_MEMBER)
        .map(|(_, len)| len)
        .expect("the fixture member");

    let mut response = serve(&r, &[]);
    assert_eq!(response.status, 200);
    assert_eq!(response.content_length(), expected);
    let body = body_bytes(&mut response);
    assert!(!body.is_empty());

    // The same member, read straight out of the committed file.
    let committed = std::fs::read(repo_root().join(REFERENCE_CONTAINER)).unwrap();
    let (offset, _) = source.member_extent(REFERENCE_MEMBER).unwrap();
    assert_eq!(
        body,
        committed[offset as usize..offset as usize + expected as usize],
        "the served bytes are exactly the member's slice of the file"
    );

    // A range over it is member-relative.
    let mut ranged = serve(&r, &[("range", "bytes=0-9")]);
    assert_eq!(ranged.status, 206);
    assert_eq!(body_bytes(&mut ranged), body[0..10]);
}

#[test]
fn a_large_member_serves_ranges_at_scale() {
    // The boundary invariants, at an offset where an off-by-one would be
    // obvious. The container is written by core's own writer and read back
    // through core's reader, so this exercises the real implementation; only
    // the member's *size* is synthetic, because it needs to be large.
    let root = temp_root("large-member");
    let container = root.join("large.mpak");
    let big: Vec<u8> = (0..1_500_000u32).map(|i| (i % 251) as u8).collect();
    let source = Members {
        manifest: format!(
            r#"{{"format":"musicpack","version":1,"album":{{"title":"Large","artists":[{{"name":"A"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"T","audio":{{"path":"audio/big.mpc","sha256":"{}"}}}}]}}]}}"#,
            sha256_hex(&big)
        ),
        table: vec![PackMember {
            path: "audio/big.mpc".into(),
            sha256_hex: sha256_hex(&big),
        }],
        content: vec![("audio/big.mpc".to_string(), big.clone())],
    };
    let mut bytes = Vec::new();
    write_mpak(&source, &mut bytes).expect("pack");
    std::fs::write(&container, &bytes).unwrap();

    let store = index(&root);
    let r = audio_ref(&store, 0);
    let (offset, length) = PackageSource::container(&container)
        .member_extent(&r.relative_path)
        .expect("extent");
    assert_eq!(length, big.len() as u64);
    assert!(offset > 0, "the member does not start the file");

    // A bounded range deep inside the member.
    let mut middle = serve(&r, &[("range", "bytes=1000000-1000999")]);
    assert_eq!(middle.status, 206);
    assert_eq!(body_bytes(&mut middle), big[1_000_000..1_001_000]);

    // The open-ended range ends at the member, not the file.
    let mut rest = serve(&r, &[("range", "bytes=1000000-")]);
    assert_eq!(rest.content_length() as usize, big.len() - 1_000_000);
    assert_eq!(body_bytes(&mut rest), big[1_000_000..]);

    // The last member byte, and not the byte that follows it in the file.
    let last = big.len() - 1;
    let mut end = serve(&r, &[("range", &format!("bytes={last}-"))]);
    assert_eq!(body_bytes(&mut end), vec![big[last]]);
    let whole = std::fs::read(&container).unwrap();
    assert!(offset + length <= whole.len() as u64);
    assert_ne!(
        whole[(offset + length) as usize],
        big[last],
        "the member does not end the file, so the boundary test is real"
    );
    // One past the member is a 416, not a read into the container index.
    let mut past = serve(&r, &[("range", &format!("bytes={}-", big.len()))]);
    assert_eq!(past.status, 416);
    assert!(body_bytes(&mut past).is_empty());
}

#[test]
fn a_large_real_container_serves_its_members() {
    // Optional: a large real container from the environment, if one is
    // provided (the repository commits none). Skips loudly otherwise, like the
    // repository's other fixture-gated tests.
    let Ok(fixture) = std::env::var("MUSICPACK_MPAK_FIXTURE") else {
        eprintln!(
            "skipping the large real-container check: set MUSICPACK_MPAK_FIXTURE \
             (no large container is committed)"
        );
        return;
    };
    let root = temp_root("large-real");
    let container = root.join("large.mpak");
    std::fs::copy(&fixture, &container).expect("copy the fixture");
    let store = index(&root);

    let source = PackageSource::container(&container);
    let track = store
        .release_media(1)
        .unwrap()
        .into_iter()
        .flat_map(|m| m.tracks)
        .next()
        .expect("a track");
    let r = store
        .resolve_track_audio(track.id)
        .unwrap()
        .expect("a servable object");
    let (offset, length) = source.member_extent(&r.relative_path).expect("extent");
    assert!(length > 100_000, "a real audio member, not a stub");

    // A window well inside the member, and the very end of it.
    let mut middle = serve(&r, &[("range", "bytes=1000-1999")]);
    assert_eq!(middle.status, 206);
    assert_eq!(body_bytes(&mut middle).len(), 1000);

    let last = length - 1;
    let mut end = serve(&r, &[("range", &format!("bytes={last}-"))]);
    assert_eq!(body_bytes(&mut end).len(), 1);

    // The bytes at the member's end really are inside the file and are not the
    // member's last byte — i.e. the assertions above are not vacuous.
    let whole = std::fs::read(&container).unwrap();
    assert!(offset + length <= whole.len() as u64);
    assert_ne!(
        whole[(offset + length) as usize],
        whole[(offset + last) as usize],
        "the member does not end the file"
    );
}
