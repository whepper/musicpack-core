//! Server `.mpak` ingestion and indexing.
//!
//! The vertical slice under test:
//!
//! ```text
//! .mpak file → musicpack-core MpakBackend → MANF → domain model
//!            → Server index → mpak:<container>#<member>
//! ```
//!
//! Fixtures are **real containers**, never a hand-rolled fake format: the
//! committed `fixtures/reference/reference-small.mpak` (produced by the
//! reference packer) covers the committed path, and a container packed at test
//! runtime with core's own writer covers a multi-track album holding real SV8
//! audio.
//!
//! The central invariant is the last line: an indexed container track must
//! resolve back to *exactly* the container and member it was indexed from. It
//! is asserted as a string shape, as a parse round-trip, and by reading the
//! member's bytes back through the same core container API the index was built
//! from.

use std::io::Read;
use std::path::{Path, PathBuf};

use musicpack_core::format::checksum::sha256_hex;
use musicpack_core::format::mpak::{PackMember, PackSource, write_mpak};
use musicpack_server::ingest::{VerifyResult, scan, verify_library};
use musicpack_server::source::{
    PackageSource, indexed_source, playback_source, resolve_playback_source,
};
use musicpack_server::store::Store;
use musicpack_server::store::sqlite::SqliteStore;

const REFERENCE_CONTAINER: &str = "fixtures/reference/reference-small.mpak";
const REFERENCE_MEMBER: &str = "audio/01.bin";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn committed_container() -> PathBuf {
    let path = repo_root().join(REFERENCE_CONTAINER);
    assert!(
        path.is_file(),
        "the committed container fixture is missing: {}",
        path.display()
    );
    path
}

fn temp_root(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("mpak-ingest-{}-{name}-{n}", std::process::id()));
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

/// A real SV8 member from the committed corpus, so the packed container holds
/// audio the server's own probe can actually decode.
fn real_audio() -> Vec<u8> {
    let path = repo_root().join("tests/fixtures/musepack/sine44-q5.mpc");
    std::fs::read(&path).unwrap_or_else(|e| panic!("fixture corpus missing: {e}"))
}

/// The manifest both source kinds are given, so the two can be compared.
fn album_manifest(audio_sha: &str, art_sha: &str, lyrics_sha: &str) -> String {
    format!(
        r#"{{"format":"musicpack","version":1,"album":{{"title":"Container Album","artists":[{{"name":"The Packer"}}],"releaseType":"album"}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"One","audio":{{"path":"audio/01.mpc","sha256":"{audio_sha}"}},"lyrics":[{{"path":"lyrics/01.lrc","sha256":"{lyrics_sha}"}}]}},{{"track":2,"title":"Two","audio":{{"path":"audio/02.mpc","sha256":"{audio_sha}"}}}}]}}],"artwork":[{{"role":"front","path":"artwork/front.jpg","sha256":"{art_sha}"}}]}}"#
    )
}

/// Packs a two-track album (real SV8 audio + artwork + lyrics) into a
/// container at `out`, returning the member bytes by path.
fn pack_album(out: &Path) -> Vec<(String, Vec<u8>)> {
    let audio = real_audio();
    let artwork: Vec<u8> = (0..2048u32).map(|i| (i % 251) as u8).collect();
    let lyrics = b"first line\nsecond line\n".to_vec();
    let content = vec![
        ("audio/01.mpc".to_string(), audio.clone()),
        ("audio/02.mpc".to_string(), audio),
        ("artwork/front.jpg".to_string(), artwork),
        ("lyrics/01.lrc".to_string(), lyrics),
    ];
    let table: Vec<PackMember> = content
        .iter()
        .map(|(path, bytes)| PackMember {
            path: path.clone(),
            sha256_hex: sha256_hex(bytes),
        })
        .collect();
    let source = Members {
        manifest: album_manifest(
            &sha256_hex(&content[0].1),
            &sha256_hex(&content[2].1),
            &sha256_hex(&content[3].1),
        ),
        table,
        content: content.clone(),
    };
    let mut bytes = Vec::new();
    write_mpak(&source, &mut bytes).expect("pack the fixture container");
    std::fs::write(out, &bytes).expect("write the fixture container");
    content
}

/// Writes the same album as a `.mpack` directory bundle, so a container and
/// its directory twin can be indexed side by side.
fn write_directory_twin(dir: &Path, members: &[(String, Vec<u8>)]) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        album_manifest(
            &sha256_hex(&members[0].1),
            &sha256_hex(&members[2].1),
            &sha256_hex(&members[3].1),
        ),
    )
    .unwrap();
    for (path, bytes) in members {
        let target = dir.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, bytes).unwrap();
    }
}

fn store() -> SqliteStore {
    SqliteStore::open(Path::new(":memory:")).expect("open an in-memory store")
}

/// Every indexed track, as `(title, member path, byte size, codec)`.
///
/// The member path and size come from the stored audio object, so this reads
/// the index itself rather than the manifest.
fn indexed_tracks(store: &SqliteStore) -> Vec<(String, String, i64, String)> {
    let mut out = Vec::new();
    for media in store.release_media(1).expect("release media") {
        for track in &media.tracks {
            let audio = store
                .resolve_track_audio(track.id)
                .expect("resolve track audio")
                .expect("the track has an audio object");
            out.push((
                track.title.clone().unwrap_or_default(),
                audio.relative_path.clone(),
                track.audio_size,
                track.codec.clone(),
            ));
        }
    }
    out
}

#[test]
fn a_container_file_is_discovered_indexed_and_projects_its_tracks() {
    let root = temp_root("index");
    let container = root.join("album.mpak");
    pack_album(&container);

    let mut store = store();
    // A *verifying* scan, because the byte layer only serves packages whose
    // `verify_status` is `valid`/`warning` — an `unverified` row is correctly
    // invisible, for containers exactly as for directory bundles.
    let result = scan(&mut store, &root, true).expect("scan the library");
    assert_eq!(result.total, 1, "the container was discovered");
    assert_eq!(result.added, 1, "exactly one source was added");
    assert_eq!(result.invalid, 0, "the container is valid");

    // The manifest drove the projection: both tracks, in manifest order, with
    // the album's own metadata, and nothing lost.
    let tracks = indexed_tracks(&store);
    assert_eq!(
        tracks
            .iter()
            .map(|(title, member, ..)| (title.as_str(), member.as_str()))
            .collect::<Vec<_>>(),
        vec![("One", "audio/01.mpc"), ("Two", "audio/02.mpc")],
    );

    let release = store.release_detail(1).unwrap().expect("a release");
    assert_eq!(release.album_title, "Container Album");
    let credits = store
        .group_credits(release.album_id)
        .expect("album credits");
    assert!(
        credits.iter().any(|c| c.name == "The Packer"),
        "the album artist came from the MANF: {credits:?}"
    );

    // The member paths are preserved verbatim and the codec came from the
    // member's real bytes (core's decoder read the SV8 header out of the
    // container), not from the extension.
    assert_eq!(tracks[0].3, "musepack-sv8", "probed from the member bytes");
    assert_eq!(tracks[0].1, "audio/01.mpc");
    assert_eq!(tracks[1].1, "audio/02.mpc");
    assert!(tracks[0].2 > 0, "the member's real byte count is indexed");

    // The package row records the container file as the locator.
    let package = store
        .package_by_path(&container.to_string_lossy())
        .expect("lookup")
        .expect("the container has a package row");
    assert_eq!(package.status, "valid");
    assert_eq!(package.fingerprint.len(), 64, "a real content fingerprint");
}

#[test]
fn every_indexed_container_track_has_a_canonical_playback_source() {
    let root = temp_root("source");
    let container = root.join("album.mpak");
    pack_album(&container);

    let mut store = store();
    scan(&mut store, &root, true).expect("scan");

    let tracks = indexed_tracks(&store);
    assert_eq!(tracks.len(), 2);
    for (_, member, _, _) in &tracks {
        // The server states a container member's source in exactly one way:
        // core's canonical formatter over the container and the member.
        let source = playback_source(&container, member);
        assert_eq!(
            source.url,
            format!("mpak:{}#{member}", container.to_string_lossy()),
            "canonical container-member key"
        );
        assert_eq!(source.kind.as_str(), "mpak");

        // The invariant: the key parses back to exactly the container and
        // member the track was indexed from.
        let (resolved_container, resolved_member) = resolve_playback_source(&source)
            .unwrap()
            .expect("a container key");
        assert_eq!(resolved_container, container.to_string_lossy());
        assert_eq!(resolved_member, *member);
    }
}

#[test]
fn the_source_of_an_indexed_track_is_derived_from_its_stored_row() {
    // The end-to-end identity guarantee: the server's own index, read back
    // through the public store seam, yields the canonical key — and that key
    // resolves to the same container and member.
    let root = temp_root("derived");
    let container = root.join("album.mpak");
    pack_album(&container);

    let mut store = store();
    scan(&mut store, &root, true).expect("scan");

    for track in store
        .release_media(1)
        .unwrap()
        .iter()
        .flat_map(|m| m.tracks.iter())
    {
        let audio = store
            .resolve_track_audio(track.id)
            .unwrap()
            .expect("an audio object");
        assert_eq!(audio.package_path, container.to_string_lossy());

        let source = indexed_source(&audio).expect("a container-backed track");
        assert_eq!(
            source.url,
            format!("mpak:{}#{}", audio.package_path, audio.relative_path)
        );
        let (c, m) = resolve_playback_source(&source).unwrap().unwrap();
        assert_eq!(c, audio.package_path);
        assert_eq!(m, audio.relative_path);
    }
}

#[test]
fn a_playback_source_resolves_to_the_exact_member_bytes() {
    // Beyond metadata: take the canonical source the server hands out,
    // resolve it, and read the member back with core's own container API — the
    // same API the index was built from.
    let root = temp_root("bytes");
    let container = root.join("album.mpak");
    let members = pack_album(&container);

    let mut store = store();
    scan(&mut store, &root, true).expect("scan");

    for track in store
        .release_media(1)
        .unwrap()
        .iter()
        .flat_map(|m| m.tracks.iter())
    {
        let audio = store.resolve_track_audio(track.id).unwrap().unwrap();
        let source = indexed_source(&audio).expect("a container source");
        let (container_path, member_path) = resolve_playback_source(&source)
            .unwrap()
            .expect("a container key");

        // Re-open exactly the container the source names, and read the member.
        let reopened = PackageSource::container(Path::new(&container_path));
        assert!(reopened.object_exists(&member_path), "{member_path} exists");
        let expected = members
            .iter()
            .find(|(p, _)| *p == member_path)
            .map(|(_, b)| b.as_slice())
            .expect("a known member");
        assert_eq!(
            reopened.object_size(&member_path),
            expected.len() as u64,
            "{member_path}: the indexed size is the member's real length"
        );
        let mut got = Vec::new();
        reopened
            .open_object(&member_path)
            .expect("open the member")
            .read_to_end(&mut got)
            .expect("read the member");
        assert_eq!(got, expected, "{member_path}: bytes read back unchanged");
    }
}

#[test]
fn the_committed_reference_container_indexes_and_resolves() {
    // A real artifact produced by the reference packer, not a container this
    // test wrote.
    let root = temp_root("committed");
    let container = root.join("reference-small.mpak");
    std::fs::copy(committed_container(), &container).expect("copy the fixture");

    let mut store = store();
    let result = scan(&mut store, &root, true).expect("scan");
    assert_eq!(result.added, 1);
    assert_eq!(result.invalid, 0);

    let tracks = indexed_tracks(&store);
    assert_eq!(tracks.len(), 1, "the fixture has one track");
    assert_eq!(tracks[0].1, REFERENCE_MEMBER);

    let release = store.release_detail(1).unwrap().unwrap();
    assert_eq!(release.album_title, "Reference Container");
    let credits = store.group_credits(release.album_id).unwrap();
    assert_eq!(credits[0].name, "Tester");

    let audio = store
        .resolve_track_audio(store.release_media(1).unwrap()[0].tracks[0].id)
        .unwrap()
        .unwrap();
    let source = indexed_source(&audio).expect("a container source");
    let (container_path, member) = resolve_playback_source(&source).unwrap().unwrap();
    assert_eq!(container_path, container.to_string_lossy());
    assert_eq!(member, REFERENCE_MEMBER);

    // Its member bytes come back intact, and really are inside the file.
    let source = PackageSource::container(Path::new(&container_path));
    let mut via_source = Vec::new();
    source
        .open_object(&member)
        .expect("open the member")
        .read_to_end(&mut via_source)
        .unwrap();
    let whole = std::fs::read(committed_container()).unwrap();
    assert!(!via_source.is_empty(), "the member is not empty");
    assert!(
        whole
            .windows(via_source.len())
            .any(|w| w == via_source.as_slice()),
        "the member's bytes really are inside the container file"
    );
}

#[test]
fn an_invalid_container_is_recorded_invalid_and_never_indexed_as_content() {
    let root = temp_root("invalid");
    // Not a container at all.
    std::fs::write(root.join("garbage.mpak"), b"this is not a container").unwrap();
    // A real container whose payload is corrupted after the header.
    let corrupt = root.join("corrupt.mpak");
    pack_album(&corrupt);
    let mut bytes = std::fs::read(&corrupt).unwrap();
    for byte in bytes.iter_mut().skip(64) {
        *byte ^= 0xff;
    }
    std::fs::write(&corrupt, &bytes).unwrap();

    let mut store = store();
    let result = scan(&mut store, &root, false).expect("scan completes");
    assert_eq!(result.total, 2, "both containers were examined");
    assert_eq!(result.added, 0, "no container became valid content");
    assert!(
        result.invalid >= 1,
        "a container that does not scan is recorded invalid: {result:?}"
    );

    // No album, release or track may exist for a container that did not scan.
    let albums = store.albums_page(10, 0, None, false).expect("albums");
    assert!(
        albums.1.is_empty(),
        "an invalid container must not project an album: {:?}",
        albums.1
    );
    for name in ["garbage.mpak", "corrupt.mpak"] {
        let path = root.join(name).to_string_lossy().into_owned();
        let row = store
            .package_by_path(&path)
            .expect("lookup")
            .expect("the container is still recorded as a source");
        assert_ne!(
            row.status, "valid",
            "{name} must never be indexed as valid content"
        );
        assert!(
            row.fingerprint.is_empty(),
            "{name} must carry no identity, so it cannot own a release"
        );
    }
}

#[test]
fn a_verifying_scan_uses_the_container_verifier() {
    // `verify` must run core's container verification, not the directory one.
    let root = temp_root("verify");
    let container = root.join("album.mpak");
    pack_album(&container);

    let mut store = store();
    scan(&mut store, &root, true).expect("a verifying scan");
    let package = store
        .package_by_path(&container.to_string_lossy())
        .unwrap()
        .unwrap();
    assert_eq!(package.status, "valid", "an intact container verifies");
    assert_eq!(package.verify_status, "valid");

    // Corrupt a member's bytes; the container verifier must catch it.
    let mut bytes = std::fs::read(&container).unwrap();
    let middle = bytes.len() / 2;
    for byte in bytes[middle..middle + 64].iter_mut() {
        *byte ^= 0xff;
    }
    std::fs::write(&container, &bytes).unwrap();

    let mut seen = VerifyResult::default();
    let verdicts = verify_library(&mut store, &root, &mut |r| {
        seen = *r;
        true
    })
    .expect("verify");
    assert_eq!(verdicts.total, 1);
    assert_eq!(
        verdicts.failed, 1,
        "a corrupted container fails: {verdicts:?}"
    );
    assert_eq!(seen.failed, 1);
}

#[test]
fn a_container_and_its_directory_twin_are_arbitrated_not_merged() {
    // The two source kinds share one library and neither disturbs the other.
    let root = temp_root("mixed");
    let container = root.join("album.mpak");
    let members = pack_album(&container);
    let dir = root.join("album.mpack");
    write_directory_twin(&dir, &members);

    let mut store = store();
    let result = scan(&mut store, &root, true).expect("scan");
    assert_eq!(result.total, 2, "both sources are discovered");
    assert_eq!(result.added, 2, "both get their own row");
    assert_eq!(result.invalid, 0);

    // Same content, so the same package fingerprint — but two distinct rows,
    // because `packages.path` is the source locator.
    let by_container = store
        .package_by_path(&container.to_string_lossy())
        .unwrap()
        .expect("container row");
    let by_dir = store
        .package_by_path(&dir.to_string_lossy())
        .unwrap()
        .expect("directory row");
    assert_ne!(by_container.id, by_dir.id);
    assert_eq!(
        by_container.fingerprint, by_dir.fingerprint,
        "identical content has an identical fingerprint"
    );

    // Two packages, one release, identical fingerprints: this is the
    // reference's **mirror** case, not an identity conflict. Both rows are
    // valid and neither is dropped, but only the first-discovered source owns
    // the release — the mirror never takes over. (Discovery sorts by path, so
    // the owner is the lexicographically first locator: `album.mpack` here,
    // since `c` < `k`.)
    assert_eq!(by_container.status, "valid");
    assert_eq!(
        by_dir.status, "valid",
        "a same-fingerprint mirror is valid, not quarantined"
    );
    let owner_audio = store
        .resolve_track_audio(store.release_media(1).unwrap()[0].tracks[0].id)
        .unwrap()
        .expect("the owning package serves the release");
    let first_by_path = if dir.as_os_str() < container.as_os_str() {
        &dir
    } else {
        &container
    };
    assert_eq!(
        owner_audio.package_path,
        first_by_path.to_string_lossy(),
        "ownership follows discovery order; the mirror does not take over"
    );
}

#[test]
fn a_directory_only_library_indexes_exactly_as_before() {
    // The regression guard for the existing path: no container anywhere, the
    // package serves bytes by locator + relative path, and it has no container
    // playback source.
    let root = temp_root("directory-only");
    let dir = root.join("album.mpack");
    let members = pack_album(&root.join("seed.mpak"));
    write_directory_twin(&dir, &members);
    std::fs::remove_file(root.join("seed.mpak")).expect("drop the seed container");

    let mut store = store();
    let result = scan(&mut store, &root, true).expect("scan");
    assert_eq!(result.total, 1);
    assert_eq!(result.added, 1);
    assert_eq!(result.invalid, 0);

    let row = store
        .package_by_path(&dir.to_string_lossy())
        .unwrap()
        .expect("the directory package");
    assert_eq!(row.status, "valid");
    assert_eq!(row.verify_status, "valid");

    let audio = store
        .resolve_track_audio(store.release_media(1).unwrap()[0].tracks[0].id)
        .unwrap()
        .expect("a servable audio object");
    assert_eq!(audio.package_path, dir.to_string_lossy());
    assert_eq!(audio.relative_path, "audio/01.mpc");
    assert!(
        indexed_source(&audio).is_none(),
        "a directory-bundle track has no container playback source"
    );
    // The bytes still come from the real file, untouched by any of this.
    let real = std::fs::read(dir.join("audio/01.mpc")).expect("the real member");
    assert_eq!(audio_size_from(&store, &audio), real.len() as i64);
}

fn audio_size_from(store: &SqliteStore, audio: &musicpack_server::store::MediaRef) -> i64 {
    store
        .release_media(1)
        .unwrap()
        .iter()
        .flat_map(|m| m.tracks.iter())
        .find(|t| Some(t.audio_id) == Some(audio.id))
        .map(|t| t.audio_size)
        .expect("the track row")
}
