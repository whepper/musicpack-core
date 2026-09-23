//! Fresh-album discovery tests: album directory → authoring draft →
//! (where relevant) the existing pipeline, end to end.
//!
//! Fixtures are built in-test and deterministic: tagged FLACs are made by
//! replacing the reference fixture's `VORBIS_COMMENT` payload, tagged
//! `.mpc` sources by appending a synthetic APEv2 tag. No legacy process
//! runs; no frozen encoder corpus is touched.

use std::path::{Path, PathBuf};

use musicpack_author::pipeline::{AuthorRequest, PipelineOptions, run};
use musicpack_author::{draft, encode, scan};
use musicpack_core::format::manifest::ReleaseType;
use musicpack_musepack_encoder::encoder::{EncoderConfig, MusepackEncoder};

// ---------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("musicpack-scan-{name}-{}", n));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
    /// A fresh album subdirectory inside this temp dir.
    fn album(&self, name: &str) -> PathBuf {
        let dir = self.0.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/reference/audio")
        .join(name)
}

/// The 16-bit FLAC fixture bytes.
fn flac_bytes() -> Vec<u8> {
    std::fs::read(fixture("flac16-44k.flac")).unwrap()
}

/// The fixture bytes with their `VORBIS_COMMENT` payload replaced by
/// `comments` (see the core tag-reader tests for the block layout).
fn flac_with_comments(comments: &[(&str, &str)]) -> Vec<u8> {
    let bytes = flac_bytes();
    assert_eq!(&bytes[0..4], b"fLaC");
    let mut offset = 4usize;
    let (payload_at, frames_at) = loop {
        let header = bytes[offset];
        let len = u32::from_be_bytes([0, bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
            as usize;
        let next = offset + 4 + len;
        if header & 0x7f == 4 {
            break (offset + 4, next);
        }
        assert_eq!(header & 0x80, 0, "fixture must contain a comment block");
        offset = next;
    };
    let mut payload = Vec::new();
    payload.extend_from_slice(&9u32.to_le_bytes());
    payload.extend_from_slice(b"musicpack");
    payload.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for (key, value) in comments {
        let entry = format!("{key}={value}");
        payload.extend_from_slice(&(entry.len() as u32).to_le_bytes());
        payload.extend_from_slice(entry.as_bytes());
    }
    let mut out = Vec::with_capacity(bytes.len() + payload.len());
    out.extend_from_slice(&bytes[..payload_at - 3]);
    let len = payload.len();
    out.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8]);
    out.extend_from_slice(&payload);
    out.extend_from_slice(&bytes[frames_at..]);
    out
}

/// A FLAC whose comment block claims an entry longer than the block.
fn flac_with_malformed_comments() -> Vec<u8> {
    let mut tagged = flac_with_comments(&[("TITLE", "Alpha")]);
    let entry_len_at = {
        let mut offset = 4usize;
        loop {
            let block_type = tagged[offset] & 0x7f;
            let len = u32::from_be_bytes([
                0,
                tagged[offset + 1],
                tagged[offset + 2],
                tagged[offset + 3],
            ]) as usize;
            if block_type == 4 {
                let vendor_len = u32::from_le_bytes([
                    tagged[offset + 4],
                    tagged[offset + 5],
                    tagged[offset + 6],
                    tagged[offset + 7],
                ]) as usize;
                break offset + 4 + 4 + vendor_len + 4;
            }
            offset += 4 + len;
        }
    };
    tagged[entry_len_at..entry_len_at + 4].copy_from_slice(&0x00FF_FFFFu32.to_le_bytes());
    tagged
}

fn write_tagged_flac(dir: &Path, name: &str, comments: &[(&str, &str)]) {
    std::fs::write(dir.join(name), flac_with_comments(comments)).unwrap();
}

/// A synthetic APEv2 tag: header + items + footer.
fn apev2_bytes(items: &[(&str, &str)]) -> Vec<u8> {
    fn footer(tag_size: u32, count: u32, flags: u32) -> Vec<u8> {
        let mut f = b"APETAGEX".to_vec();
        f.extend_from_slice(&2000u32.to_le_bytes());
        f.extend_from_slice(&tag_size.to_le_bytes());
        f.extend_from_slice(&count.to_le_bytes());
        f.extend_from_slice(&flags.to_le_bytes());
        f.extend_from_slice(&[0u8; 8]);
        f
    }
    let mut body = Vec::new();
    for (key, value) in items {
        body.extend_from_slice(&(value.len() as u32).to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes()); // text item
        body.extend_from_slice(key.as_bytes());
        body.push(0);
        body.extend_from_slice(value.as_bytes());
    }
    let tag_size = (64 + body.len()) as u32;
    let count = items.len() as u32;
    let mut out = footer(tag_size, count, 0x2000_0000 | 0x4000_0000);
    out.extend_from_slice(&body);
    out.extend_from_slice(&footer(tag_size, count, 0x8000_0000 | 0x4000_0000));
    out
}

fn scan_draft(dir: &Path) -> draft::Draft {
    let bytes = scan::source_to_draft(dir).unwrap();
    draft::parse(bytes.as_bytes()).unwrap()
}

fn track_numbers(d: &draft::Draft) -> Vec<i32> {
    d.media
        .iter()
        .flat_map(|m| m.tracks.iter().map(|t| t.number))
        .collect()
}

fn track_titles(d: &draft::Draft) -> Vec<&str> {
    d.media
        .iter()
        .flat_map(|m| m.tracks.iter().map(|t| t.title.as_str()))
        .collect()
}

fn track_paths(d: &draft::Draft) -> Vec<&str> {
    d.media
        .iter()
        .flat_map(|m| m.tracks.iter().map(|t| t.audio_path.as_str()))
        .collect()
}

// ---------------------------------------------------------------------
// normal albums, metadata, ordering
// ---------------------------------------------------------------------

const ALBUM_TAGS: &[(&str, &str)] = &[
    ("ALBUM", "Tagged Album"),
    ("ALBUMARTIST", "The Band"),
    ("DATE", "2021"),
    ("RELEASETYPE", "compilation"),
    ("MUSICBRAINZ_RELEASEID", "rel-0001"),
    ("MUSICBRAINZ_RELEASEGROUPID", "rg-0001"),
    ("BARCODE", "1234567890"),
    ("CATALOGNUMBER", "CAT-001"),
];

#[test]
fn discovers_a_tagged_stereo_album() {
    let temp = TempDir::new("tagged");
    let album = temp.album("Tagged Album");
    let mut first: Vec<(&str, &str)> = ALBUM_TAGS.to_vec();
    first.extend([
        ("TITLE", "Alpha"),
        ("TRACKNUMBER", "1"),
        ("ARTIST", "A One"),
        ("GENRE", "Electronic"),
        ("GENRE", "Ambient"),
        ("ISRC", "USABC2100001"),
        ("MUSICBRAINZ_RECORDINGID", "rec-0001"),
    ]);
    write_tagged_flac(&album, "01 - Alpha.flac", &first);
    let mut second: Vec<(&str, &str)> = ALBUM_TAGS.to_vec();
    second.extend([
        ("TITLE", "Beta"),
        ("TRACKNUMBER", "2"),
        ("ARTIST", "A Two"),
        ("COMPOSER", "The Composer"),
        // A label tag that only the *later* file carries: the union must
        // not let an absent first file shadow it.
        ("LABEL", "Later Label"),
    ]);
    write_tagged_flac(&album, "02 - Beta.flac", &second);

    std::fs::write(album.join("cover.jpg"), b"jpeg-bytes").unwrap();
    std::fs::write(album.join("booklet.pdf"), b"pdf-bytes").unwrap();
    std::fs::write(album.join("notes.txt"), b"notes").unwrap();
    std::fs::write(album.join("01 - Alpha.lrc"), b"[00:01.00]line\n").unwrap();
    std::fs::write(album.join("extra.lrc"), b"[00:01.00]stray\n").unwrap();

    let d = scan_draft(&album);
    assert_eq!(
        d.source_root,
        album.canonicalize().unwrap(),
        "sourceRoot is the canonical album directory"
    );

    // album metadata
    assert_eq!(d.album.title, "Tagged Album");
    assert_eq!(d.album.artists.len(), 1);
    assert_eq!(d.album.artists[0].name, "The Band");
    assert_eq!(d.album.artists[0].role.as_deref(), Some("main"));
    assert_eq!(d.album.release_type, Some(ReleaseType::Compilation));
    assert_eq!(d.album.genres, vec!["Electronic", "Ambient"]);

    // release + identifiers (first-wins union across files)
    let release = d.release.as_ref().expect("release from DATE/LABEL tags");
    assert_eq!(release.release_date.as_deref(), Some("2021"));
    assert_eq!(release.label.as_deref(), Some("Later Label"));
    assert_eq!(release.catalogue_number.as_deref(), Some("CAT-001"));
    let ids = d.identifiers.as_ref().expect("MB/barcode identifiers");
    assert_eq!(ids.musicbrainz_release_id.as_deref(), Some("rel-0001"));
    assert_eq!(ids.musicbrainz_release_group_id.as_deref(), Some("rg-0001"));
    assert_eq!(ids.barcode.as_deref(), Some("1234567890"));

    // tracks
    assert_eq!(d.media.len(), 1);
    let tracks = &d.media[0].tracks;
    assert_eq!(tracks.len(), 2);
    assert_eq!((tracks[0].number, tracks[0].title.as_str()), (1, "Alpha"));
    assert_eq!(tracks[0].audio_path, "01 - Alpha.flac");
    let t1_ids = tracks[0].identifiers.as_ref().expect("track identifiers");
    assert_eq!(t1_ids.isrc.as_deref(), Some("USABC2100001"));
    assert_eq!(t1_ids.musicbrainz_recording_id.as_deref(), Some("rec-0001"));
    assert_eq!((tracks[1].number, tracks[1].title.as_str()), (2, "Beta"));
    assert_eq!(tracks[1].artists.len(), 2, "ARTIST + COMPOSER credits");
    assert_eq!(tracks[1].artists[0].name, "A Two");
    assert_eq!(tracks[1].artists[0].role.as_deref(), Some("main"));
    assert_eq!(tracks[1].artists[1].name, "The Composer");
    assert_eq!(tracks[1].artists[1].role.as_deref(), Some("composer"));

    // assets
    assert_eq!(d.artwork.len(), 1);
    assert_eq!(d.artwork[0].role, "front");
    assert_eq!(d.artwork[0].path.as_deref(), Some("cover.jpg"));
    assert_eq!(d.booklet, vec!["booklet.pdf"]);
    assert_eq!(d.extras, vec!["notes.txt"]);
    // matched sidecar → per-track; stray sidecar → root lyrics.
    assert_eq!(tracks[0].lyrics.len(), 1);
    assert_eq!(tracks[0].lyrics[0].path, "01 - Alpha.lrc");
    assert!(tracks[1].lyrics.is_empty());
    assert_eq!(d.lyrics, vec!["extra.lrc"]);

    let report = draft::validate(&d);
    assert!(report.is_ok(), "a fully tagged album validates: {report:?}");
}

#[test]
fn orders_tracks_numerically_and_deterministically() {
    let temp = TempDir::new("order");
    let album = temp.album("Order Album");
    // No leading zeros: lexicographic name order would put "10" first.
    std::fs::copy(fixture("flac16-44k.flac"), album.join("10 - Ten.flac")).unwrap();
    std::fs::copy(fixture("flac16-44k.flac"), album.join("2 - Two.flac")).unwrap();
    std::fs::copy(fixture("flac16-44k.flac"), album.join("09 - Nine.flac")).unwrap();

    let d = scan_draft(&album);
    assert_eq!(
        track_numbers(&d),
        vec![2, 9, 10],
        "numeric, not lexicographic"
    );
    assert_eq!(track_titles(&d), vec!["Two", "Nine", "Ten"]);

    // Deterministic output: a second scan of the same tree is identical.
    let first = scan::source_to_draft(&album).unwrap();
    let second = scan::source_to_draft(&album).unwrap();
    assert_eq!(first, second, "discovery must be deterministic");
}

#[test]
fn renumbers_unnumbered_discs_in_path_order() {
    let temp = TempDir::new("unnumbered");
    let album = temp.album("Unnumbered Album");
    std::fs::copy(fixture("flac16-44k.flac"), album.join("z.flac")).unwrap();
    std::fs::copy(fixture("flac16-44k.flac"), album.join("a.flac")).unwrap();

    let d = scan_draft(&album);
    assert_eq!(track_paths(&d), vec!["a.flac", "z.flac"]);
    assert_eq!(track_numbers(&d), vec![1, 2]);
    assert_eq!(track_titles(&d), vec!["a", "z"]);
}

#[test]
fn numbers_discs_from_directories_and_tags() {
    let temp = TempDir::new("discs");
    let album = temp.album("Two Disc Album");
    let cd1 = album.join("CD1");
    let cd2 = album.join("CD2");
    std::fs::create_dir_all(&cd1).unwrap();
    std::fs::create_dir_all(&cd2).unwrap();

    write_tagged_flac(&album, "01 - One.flac", &[("TRACKNUMBER", "1")]);
    // In CD1 but tagged DISCNUMBER=2: the directory name wins.
    write_tagged_flac(
        &cd1,
        "02 - CDFirst.flac",
        &[("TRACKNUMBER", "2"), ("DISCNUMBER", "2")],
    );
    // In CD2 but tagged DISCNUMBER=1: the directory name wins again.
    write_tagged_flac(
        &cd2,
        "01 - Two.flac",
        &[("TRACKNUMBER", "1"), ("DISCNUMBER", "1")],
    );
    // A root file takes its disc from the tag.
    write_tagged_flac(
        &album,
        "03 - Three.flac",
        &[("TRACKNUMBER", "2"), ("DISCNUMBER", "2")],
    );

    let d = scan_draft(&album);
    assert_eq!(d.media.len(), 2, "two discs discovered");
    assert_eq!(d.media[0].number, 1);
    assert_eq!(
        track_paths_of(&d, 0),
        vec!["01 - One.flac", "CD1/02 - CDFirst.flac"]
    );
    assert_eq!(numbers_of(&d, 0), vec![1, 2]);
    assert_eq!(d.media[1].number, 2);
    // Disc 2's numbers are unique and positive → kept, and the stored
    // order is the sorted (number, path) order.
    assert_eq!(numbers_of(&d, 1), vec![1, 2]);
    assert_eq!(
        track_paths_of(&d, 1),
        vec!["CD2/01 - Two.flac", "03 - Three.flac"],
        "sorted by track number within the disc"
    );
}

fn track_paths_of(d: &draft::Draft, disc: usize) -> Vec<&str> {
    d.media[disc]
        .tracks
        .iter()
        .map(|t| t.audio_path.as_str())
        .collect()
}

fn numbers_of(d: &draft::Draft, disc: usize) -> Vec<i32> {
    d.media[disc].tracks.iter().map(|t| t.number).collect()
}

#[test]
fn duplicate_track_numbers_are_renumbered_deterministically() {
    let temp = TempDir::new("dup");
    let album = temp.album("Duplicate Album");
    write_tagged_flac(&album, "b.flac", &[("TRACKNUMBER", "1")]);
    write_tagged_flac(&album, "a.flac", &[("TRACKNUMBER", "1")]);

    let d = scan_draft(&album);
    assert_eq!(track_paths(&d), vec!["a.flac", "b.flac"], "path tiebreak");
    assert_eq!(track_numbers(&d), vec![1, 2], "duplicates renumbered");
}

// ---------------------------------------------------------------------
// metadata edge cases
// ---------------------------------------------------------------------

#[test]
fn missing_metadata_falls_back_to_names_and_validates_closed() {
    let temp = TempDir::new("bare");
    let album = temp.album("Bare Album");
    // The raw fixture carries only its original `encoder=` comment.
    std::fs::copy(fixture("flac16-44k.flac"), album.join("01 - Raw.flac")).unwrap();

    let d = scan_draft(&album);
    assert_eq!(d.album.title, "Bare Album", "directory name is the title");
    assert!(d.album.artists.is_empty(), "no artist tags to fall back on");
    assert_eq!(track_titles(&d), vec!["Raw"], "filename-derived title");

    // Fail-closed: the validator surfaces the missing credit instead of
    // inventing one.
    let report = draft::validate(&d);
    assert!(!report.is_ok());
    assert!(
        report.errors.iter().any(|e| e.contains("no artist")),
        "errors: {report:?}"
    );
}

#[test]
fn artist_only_albums_fall_back_to_track_artists() {
    let temp = TempDir::new("artist-fallback");
    let album = temp.album("Solo Album");
    write_tagged_flac(&album, "01 - Song.flac", &[("ARTIST", "The Soloist")]);

    let d = scan_draft(&album);
    assert_eq!(d.album.artists.len(), 1);
    assert_eq!(d.album.artists[0].name, "The Soloist");
    let report = draft::validate(&d);
    assert!(report.is_ok(), "{report:?}");
}

#[test]
fn malformed_metadata_still_discovers_the_file() {
    let temp = TempDir::new("malformed");
    let album = temp.album("Malformed Album");
    std::fs::write(
        album.join("01 - Broken.flac"),
        flac_with_malformed_comments(),
    )
    .unwrap();

    // Tag reading is best-effort: the file is discovered with
    // filename-derived fields rather than dropped or failing the scan.
    let d = scan_draft(&album);
    assert_eq!(track_paths(&d), vec!["01 - Broken.flac"]);
    assert_eq!(track_titles(&d), vec!["Broken"]);
    assert_eq!(d.album.title, "Malformed Album");
}

// ---------------------------------------------------------------------
// source formats, paths, failures
// ---------------------------------------------------------------------

#[test]
fn rejects_directories_without_buildable_audio() {
    let temp = TempDir::new("no-audio");
    let album = temp.album("No Audio");
    std::fs::write(album.join("song.ogg"), b"OggS").unwrap();
    std::fs::write(album.join("track.mp3"), b"ID3").unwrap();

    let err = scan::source_to_draft(&album).unwrap_err();
    match err {
        musicpack_author::AuthorError::Io { detail } => {
            assert!(detail.contains("no audio files found"), "detail: {detail}");
        }
        other => panic!("expected Io, got {other:?}"),
    }
}

#[test]
fn discovers_uppercase_extensions_case_insensitively() {
    let temp = TempDir::new("case");
    let album = temp.album("Uppercase Album");
    std::fs::copy(fixture("flac16-44k.flac"), album.join("01 - Upper.FLAC")).unwrap();
    std::fs::write(album.join("COVER.JPG"), b"jpeg").unwrap();

    let d = scan_draft(&album);
    assert_eq!(track_paths(&d), vec!["01 - Upper.FLAC"]);
    assert_eq!(d.artwork[0].path.as_deref(), Some("COVER.JPG"));
}

#[test]
fn invalid_directories_fail_closed() {
    let temp = TempDir::new("invalid");

    // A file is not a directory.
    let file = temp.path().join("not-a-dir.flac");
    std::fs::write(&file, b"fLaC").unwrap();
    let err = scan::source_to_draft(&file).unwrap_err();
    assert!(err.to_string().contains("is not a directory"), "err: {err}");

    // A missing path cannot be resolved.
    let err = scan::source_to_draft(&temp.path().join("missing")).unwrap_err();
    assert!(err.to_string().contains("cannot resolve"), "err: {err}");

    // An empty directory has no audio.
    let empty = temp.album("Empty");
    let err = scan::source_to_draft(&empty).unwrap_err();
    assert!(
        err.to_string().contains("no audio files found"),
        "err: {err}"
    );
}

#[test]
fn missing_audio_after_discovery_fails_validation() {
    let temp = TempDir::new("missing");
    let album = temp.album("Missing Album");
    write_tagged_flac(&album, "01 - Gone.flac", &[("TITLE", "Gone")]);
    let bytes = scan::source_to_draft(&album).unwrap();

    std::fs::remove_file(album.join("01 - Gone.flac")).unwrap();
    let d = draft::parse(bytes.as_bytes()).unwrap();
    let report = draft::validate(&d);
    assert!(!report.is_ok());
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("audio file not found")),
        "errors: {report:?}"
    );
}

// ---------------------------------------------------------------------
// artwork
// ---------------------------------------------------------------------

#[test]
fn discovers_one_deterministic_front_cover() {
    let temp = TempDir::new("artwork");
    let root = temp.album("Cover Album");
    // Name priority: cover beats folder, regardless of listing order.
    std::fs::write(root.join("folder.png"), b"png").unwrap();
    std::fs::write(root.join("cover.jpg"), b"jpg").unwrap();
    write_tagged_flac(&root, "01 - One.flac", &[("TRACKNUMBER", "1")]);
    let d = scan_draft(&root);
    assert_eq!(d.artwork.len(), 1);
    assert_eq!(d.artwork[0].path.as_deref(), Some("cover.jpg"));

    // Root wins over a disc directory's cover.
    let nested = temp.album("Nested Cover Album");
    let cd1 = nested.join("CD1");
    std::fs::create_dir_all(&cd1).unwrap();
    std::fs::write(cd1.join("cover.jpg"), b"nested").unwrap();
    std::fs::write(nested.join("front.png"), b"root").unwrap();
    write_tagged_flac(&cd1, "01 - One.flac", &[("TRACKNUMBER", "1")]);
    let d = scan_draft(&nested);
    assert_eq!(d.artwork[0].path.as_deref(), Some("front.png"));

    // Only a disc-directory cover → discovered from the disc dir.
    let disc_only = temp.album("Disc Cover Album");
    let cd2 = disc_only.join("CD2");
    std::fs::create_dir_all(&cd2).unwrap();
    std::fs::write(cd2.join("folder.jpeg"), b"jpeg").unwrap();
    write_tagged_flac(&cd2, "01 - One.flac", &[("TRACKNUMBER", "1")]);
    let d = scan_draft(&disc_only);
    assert_eq!(d.artwork[0].path.as_deref(), Some("CD2/folder.jpeg"));

    // No cover anywhere → empty artwork (validation warns, does not fail).
    let bare = temp.album("No Cover Album");
    write_tagged_flac(
        &bare,
        "01 - One.flac",
        &[("TRACKNUMBER", "1"), ("ALBUMARTIST", "The Band")],
    );
    let d = scan_draft(&bare);
    assert!(d.artwork.is_empty());
    let report = draft::validate(&d);
    assert!(report.is_ok(), "{report:?}");
    assert!(
        report.warnings.iter().any(|w| w.contains("artwork")),
        "missing artwork must be a warning: {report:?}"
    );
}

// ---------------------------------------------------------------------
// .mpc pass-through with APEv2 tags
// ---------------------------------------------------------------------

#[test]
fn mpc_pass_through_reads_apev2_tags() {
    let temp = TempDir::new("mpc");
    let album = temp.album("Sine Album");
    let mut mpc = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/musepack/sine44-q5.mpc"),
    )
    .unwrap();
    mpc.extend(apev2_bytes(&[
        ("Title", "Q5 Sine"),
        ("Album", "Tagged Sine Album"),
        ("Artist", "The Synth"),
        ("Track", "4"),
    ]));
    std::fs::write(album.join("04 - Q5 Sine.mpc"), mpc).unwrap();

    let d = scan_draft(&album);
    assert_eq!(track_paths(&d), vec!["04 - Q5 Sine.mpc"]);
    assert_eq!(track_titles(&d), vec!["Q5 Sine"], "APEv2 Title wins");
    assert_eq!(track_numbers(&d), vec![4], "APEv2 Track wins");
    assert_eq!(d.album.title, "Tagged Sine Album");
    assert_eq!(d.album.artists[0].name, "The Synth");
    let report = draft::validate(&d);
    assert!(report.is_ok(), "{report:?}");
}

// ---------------------------------------------------------------------
// source fidelity + the full directory → package workflow
// ---------------------------------------------------------------------

#[test]
fn discovery_preserves_24bit_precision_through_the_wide_path() {
    let temp = TempDir::new("wide");
    let album = temp.album("Wide Album");
    std::fs::copy(fixture("wav24-44k.wav"), album.join("01 - Wide.wav")).unwrap();

    let d = scan_draft(&album);
    let source = album.join(&d.media[0].tracks[0].audio_path);
    let out = temp.path().join("wide.mpc");
    encode::encode_to(&source, &out, 6.0).expect("24-bit encode");
    let author_bytes = std::fs::read(&out).unwrap();

    // The direct wide replay at the source's own format must agree
    // byte-for-byte — discovery never degrades precision.
    let mut decoder =
        musicpack_core::audio::open(Box::new(std::fs::File::open(&source).unwrap())).unwrap();
    let info = decoder.info().clone();
    assert_eq!((info.bits_per_sample, info.channels), (24, 2));
    let mut pcm = vec![0i32; info.total_frames.expect("known length") as usize * 2];
    let frames = decoder.read_s32(&mut pcm).unwrap();
    pcm.truncate(frames * 2);
    let config = EncoderConfig::new(6.0, info.sample_rate, info.channels as u32);
    let direct = MusepackEncoder::new(config)
        .unwrap()
        .encode_s32(&pcm)
        .unwrap();
    assert_eq!(author_bytes, direct);
}

#[test]
fn full_directory_to_package_workflow() {
    let temp = TempDir::new("e2e-scan");
    let album = temp.album("Workflow Album");
    write_tagged_flac(
        &album,
        "01 - Alpha.flac",
        &[
            ("TITLE", "Alpha"),
            ("TRACKNUMBER", "1"),
            ("ALBUM", "Workflow Album"),
            ("ALBUMARTIST", "The Band"),
            ("DATE", "2020"),
            ("ISRC", "USABC2000001"),
        ],
    );
    write_tagged_flac(
        &album,
        "02 - Beta.flac",
        &[
            ("TITLE", "Beta"),
            ("TRACKNUMBER", "2"),
            ("ALBUM", "Workflow Album"),
            ("ALBUMARTIST", "The Band"),
            ("DATE", "2020"),
        ],
    );
    std::fs::write(album.join("cover.jpg"), b"jpg").unwrap();
    std::fs::write(album.join("01 - Alpha.lrc"), b"[00:01.00]line\n").unwrap();

    // directory → draft
    let bytes = scan::source_to_draft(&album).unwrap();
    let d = draft::parse(bytes.as_bytes()).unwrap();
    let report = draft::validate(&d);
    assert!(report.is_ok(), "discovered draft validates: {report:?}");

    // draft → package through the existing pipeline (encode + waveform +
    // loudness + build + core verify).
    let output = temp.path().join("Workflow Album.mpack");
    let outcome = run(&AuthorRequest {
        draft_json: bytes.as_bytes(),
        output: &output,
        options: PipelineOptions::default(),
        identify: None,
    })
    .expect("pipeline run");
    assert!(outcome.report.is_ok(), "{:?}", outcome.report.findings());
    assert!(output.join("manifest.json").is_file());

    // The manifest carries the tag-derived metadata and encoded audio.
    assert_eq!(outcome.manifest.album.title, "Workflow Album");
    assert_eq!(outcome.manifest.album.artists[0].name, "The Band");
    assert_eq!(
        outcome.manifest.media[0].tracks[0].audio_codec.as_deref(),
        Some("musepack-sv8")
    );
    assert_eq!(
        outcome.manifest.media[0].tracks[0].audio.path,
        "audio/01 - Alpha.mpc"
    );
    assert_eq!(outcome.manifest.media[0].tracks[0].lyrics.len(), 1);
    assert!(output.join("artwork/front.jpg").is_file());
    assert!(output.join("analysis/waveform/01-01.wfm").is_file());
}
