//! Ingestion oracle: the Rust scan must produce the same observable
//! `library.db` state as the legacy C scan, round after round.
//!
//! Method: for each scenario, two identical scratch libraries are built
//! (one per implementation) from the same builders below. Fresh databases
//! go through `musicpack-server scan` (C binary, via
//! `MUSICPACK_LEGACY_SERVER`) and [`musicpack_server::ingest::scan`]
//! (Rust). Both databases are dumped, normalized (timestamps, uids and scan
//! tokens redacted by shape; row ids remapped through natural keys so
//! discovery-order differences cannot hide value divergences), and compared
//! as strings. Scan counters are compared against the C binary's summary
//! line.
//!
//! Rounds carry database state forward: each round applies the same
//! filesystem mutation to both libraries, rescans both, and compares
//! again. This exercises the fast paths, the sweep, ownership transfer and
//! ID stability exactly as production would.
//!
//! Without `MUSICPACK_LEGACY_SERVER`, the C side is skipped with an
//! explicit notice and the Rust-side assertions (counters, state machine,
//! stability) still run.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

use musicpack_core::format::checksum;
use musicpack_server::ingest::{ScanResult, scan};
use musicpack_server::store::sqlite::SqliteStore;

// ---- fixture builders ----------------------------------------------------

fn test_root(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("oracle-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sha_hex(bytes: &[u8]) -> String {
    checksum::sha256_hex(bytes)
}

/// Writes `rel` under `dir` with `content`, returning its SHA-256 hex.
fn write_file(dir: &Path, rel: &str, content: &[u8]) -> String {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, content).unwrap();
    sha_hex(content)
}

fn json_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

fn real_mpc_bytes() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/musepack/sine44-q5.mpc"
    ))
    .unwrap()
}

fn real_flac_bytes() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/reference/audio/flac16-44k.flac"
    ))
    .unwrap()
}

/// The rich basic package: two discs, three tracks (real Musepack + real
/// FLAC + placeholder bytes), artists with roles/sort names/MBIDs,
/// artwork/booklet/lyrics/extras, waveforms, one representation, loudness,
/// MB identifiers, genres (incl. escaping edge cases) and full
/// source/identity/provenance blocks.
fn build_basic(lib: &Path, name: &str) -> PathBuf {
    let dir = lib.join(name);
    let mpc_sha = write_file(&dir, "audio/01.mpc", &real_mpc_bytes());
    let flac_sha = write_file(&dir, "audio/02.flac", &real_flac_bytes());
    let wav_sha = write_file(&dir, "audio/03.wav", b"placeholder-wav");
    let rep_sha = write_file(&dir, "audio/01-rep.flac", &real_flac_bytes());
    let front_sha = write_file(&dir, "artwork/front.jpg", b"front-image");
    let book_sha = write_file(&dir, "booklet/booklet.pdf", b"booklet-pdf");
    let lyr_sha = write_file(&dir, "lyrics/01.lrc", b"lyrics");
    let ext_sha = write_file(&dir, "extras/notes.txt", b"extra-notes");
    let wfm1_sha = write_file(&dir, "waveform/01.wfm", &[0x80u8; 20]);
    let wfm2_sha = write_file(&dir, "waveform/02.wfm", &[0x80u8; 40]);
    let manifest = format!(
        concat!(
            r#"{{"format":"musicpack","version":1,"#,
            r#""album":{{"title":"Basic Älbum","artists":[{{"name":"Alice","role":"vocals","sortName":"Alice, A","musicbrainzId":"11111111-2222-3333-4444-555555555555"}},{{"name":"Bob","role":"guitar"}}],"releaseType":"album","originalReleaseDate":"2024-05-01","genres":["rock","a\"b","c\\d"]}},"#,
            r#""identifiers":{{"musicbrainzReleaseGroupId":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee","musicbrainzReleaseId":"ffffffff-1111-2222-3333-444444444444","barcode":"012345678905"}},"#,
            r#""release":{{"edition":"Deluxe","releaseDate":"2024-05-01","country":"US","label":"Probe Records","catalogueNumber":"PROBE-001","notes":"liner notes"}},"#,
            r#""source":{{"kind":"cd-rip","store":"Probe Store","id":"ps-1"}},"#,
            r#""identity":{{"source":"musicbrainz","confidence":"exact"}},"#,
            r#""provenance":{{"tool":"probe-tool","toolVersion":"0.1"}},"#,
            r#""loudness":{{"algorithm":"ITU-R BS.1770-5","albumLUFS":-8.5,"albumTruePeakDbTP":-1.25}},"#,
            r#""artwork":[{{"role":"front","path":"artwork/front.jpg","sha256":"{front_sha}"}}],"#,
            r#""booklet":[{{"path":"booklet/booklet.pdf","sha256":"{book_sha}"}}],"#,
            r#""lyrics":[{{"path":"lyrics/01.lrc","sha256":"{lyr_sha}"}}],"#,
            r#""extras":[{{"path":"extras/notes.txt","sha256":"{ext_sha}"}}],"#,
            r#""media":["#,
            r#"{{"disc":1,"format":"Digital","title":"Disc One","tracks":["#,
            r#"{{"track":1,"title":"First","duration":180.5,"loudness":{{"trackLUFS":-7.5,"truePeakDbTP":-1.0}},"artists":[{{"name":"Alice"}}],"identifiers":{{"isrc":"US-AAA-24-00001"}},"source":{{"store":"Probe Store","trackId":"t1"}},"sourceAudio":{{"codec":"flac","md5":"d41d8cd98f00b204e9800998ecf8427e"}},"audio":{{"path":"audio/01.mpc","sha256":"{mpc_sha}"}},"waveform":{{"version":1,"path":"waveform/01.wfm","sha256":"{wfm1_sha}","intervalMs":100,"encoding":"peak-rms-u8","floorDb":-60,"points":10}},"representations":[{{"path":"audio/01-rep.flac","sha256":"{rep_sha}","label":"FLAC 16/44","codec":"flac"}}]}},"#,
            r#"{{"track":2,"title":"Second","audio":{{"path":"audio/02.flac","sha256":"{flac_sha}"}},"waveform":{{"version":1,"path":"waveform/02.wfm","sha256":"{wfm2_sha}","intervalMs":100,"encoding":"peak-rms-u8","floorDb":-60,"points":20}}}}]}},"#,
            r#"{{"disc":2,"tracks":["#,
            r#"{{"track":1,"title":"Third","audio":{{"path":"audio/03.wav","sha256":"{wav_sha}"}}}}]}}]}}"#
        ),
        front_sha = front_sha,
        book_sha = book_sha,
        lyr_sha = lyr_sha,
        ext_sha = ext_sha,
        mpc_sha = mpc_sha,
        flac_sha = flac_sha,
        wav_sha = wav_sha,
        rep_sha = rep_sha,
        wfm1_sha = wfm1_sha,
        wfm2_sha = wfm2_sha,
    );
    std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    dir
}

/// A minimal package with one placeholder track. `extra` is spliced into
/// the manifest root (for identifiers/release blocks).
fn build_minimal(lib: &Path, name: &str, title: &str, artist: &str, extra: &str) -> PathBuf {
    let dir = lib.join(name);
    let audio_sha = write_file(&dir, "audio/01.mpc", b"placeholder");
    let manifest = format!(
        concat!(
            r#"{{"format":"musicpack","version":1,"#,
            r#""album":{{"title":"{title}","artists":[{{"name":"{artist}"}}]}},"#,
            r#""media":[{{"disc":1,"tracks":[{{"track":1,"title":"T","audio":{{"path":"audio/01.mpc","sha256":"{audio_sha}"}}}}]}}]{extra}}}"#
        ),
        title = json_escape(title),
        artist = json_escape(artist),
        audio_sha = audio_sha,
        extra = extra,
    );
    std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    dir
}

// ---- oracle harness ------------------------------------------------------

/// One side of a comparison: a scratch library plus its database.
struct Side {
    lib: PathBuf,
    db: PathBuf,
}

fn fresh_side(root: &Path, tag: &str) -> Side {
    let lib = root.join(format!("lib-{tag}"));
    std::fs::create_dir_all(&lib).unwrap();
    Side {
        lib,
        db: root.join(format!("{tag}.db")),
    }
}

/// Parsed C summary line: `scan: T packages (A added, U updated, M moved,
/// R removed, I invalid)`.
#[derive(Debug, PartialEq, Eq)]
struct CSummary {
    total: usize,
    added: usize,
    updated: usize,
    moved: usize,
    removed: usize,
    invalid: usize,
}

/// Waits for the epoch-second clock to advance. The legacy CLI is a new
/// process per invocation, so its scan-token counter restarts at 0 every
/// time: two C scans inside the same second mint the SAME token
/// (`s<epoch>.0`), and the sweep then sees no stale rows. The Rust
/// `scan()` advances its counter in-process (like the C server process),
/// so without this guard the two implementations would compare a real
/// sweep against a token-collision no-op. This preserves genuine C
/// semantics instead of papering over them.
fn wait_for_next_second() {
    use std::time::{SystemTime, UNIX_EPOCH};
    let start = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    while SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        == start
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn scan_c(binary: &Path, lib: &Path, db: &Path) -> CSummary {
    wait_for_next_second();
    let output = Command::new(binary)
        .arg("scan")
        .arg("--library")
        .arg(lib)
        .arg("--database")
        .arg(db)
        .env_remove("MUSICPACK_LOG")
        .output()
        .expect("failed to run the legacy server binary");
    assert!(
        output.status.success(),
        "C scan failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    parse_c_summary(&String::from_utf8_lossy(&output.stdout))
}

fn parse_c_summary(stdout: &str) -> CSummary {
    // `scan: 20 packages (20 added, 0 updated, 0 moved, 0 removed, 0 invalid)`
    let line = stdout
        .lines()
        .find(|l| l.starts_with("scan: "))
        .expect("C summary line missing");
    let nums: Vec<usize> = line
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap())
        .collect();
    assert_eq!(nums.len(), 6, "unexpected C summary shape: {line}");
    CSummary {
        total: nums[0],
        added: nums[1],
        updated: nums[2],
        moved: nums[3],
        removed: nums[4],
        invalid: nums[5],
    }
}

fn scan_rust(lib: &Path, db: &Path) -> ScanResult {
    let mut store = SqliteStore::open(db).unwrap();
    scan(&mut store, lib, false).unwrap()
}

fn scan_rust_verify(lib: &Path, db: &Path) -> ScanResult {
    let mut store = SqliteStore::open(db).unwrap();
    scan(&mut store, lib, true).unwrap()
}

/// Dumps every table of `db` in id order as `(columns, rows)`.
fn dump_db(db: &Path) -> BTreeMap<String, (Vec<String>, Vec<Vec<String>>)> {
    let conn = rusqlite::Connection::open(db).unwrap();
    let tables: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut out = BTreeMap::new();
    for table in tables {
        let mut cols: Vec<String> = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        // v11 comparison boundary (docs/musicpack-lyrics-v1.md §7.1): the
        // C schema (v10) cannot represent `assets.track_id`/`lang`, so the
        // differential projects the assets table onto the C-visible
        // columns and the package-level rows (`track_id IS NULL`) —
        // exactly the graph the C could have written. Track-linked rows
        // are asserted Rust-side (`tests/lyrics_server.rs`). The filter
        // applies only to v11 databases (a v10 C database has no such
        // column; all its rows are package-level by construction).
        let mut filter = String::new();
        if table == "assets" && cols.iter().any(|c| c == "track_id") {
            cols.retain(|c| c != "track_id" && c != "lang");
            filter.push_str(" WHERE track_id IS NULL");
        }
        // Tables without an `id` column (group_artists, track_artists,
        // schema_version) order by rowid instead.
        let order = if cols.contains(&"id".to_string()) {
            cols.iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            "*".to_string()
        };
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {order} FROM {table}{filter} ORDER BY rowid"
            ))
            .unwrap();
        let mut query = stmt.query([]).unwrap();
        let mut rows: Vec<Vec<String>> = Vec::new();
        while let Some(row) = query.next().unwrap() {
            rows.push(read_row(row, cols.len()));
        }
        out.insert(table, (cols, rows));
    }
    out
}

fn read_row(row: &rusqlite::Row<'_>, n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            let value: rusqlite::types::Value = row.get(i).unwrap();
            match value {
                rusqlite::types::Value::Null => "NULL".to_string(),
                rusqlite::types::Value::Integer(v) => v.to_string(),
                rusqlite::types::Value::Real(v) => format!("{v:?}"),
                rusqlite::types::Value::Text(v) => format!("T:{v}"),
                rusqlite::types::Value::Blob(v) => format!("B:{}", checksum::sha256_hex(&v)),
            }
        })
        .collect()
}

/// Normalizes one dumped database into a comparable string:
/// - `created_at`/`updated_at`/`last_scan`/`applied_at` → shape-asserted
///   placeholders (wall-clock values by design);
/// - `uid` → shape-asserted placeholder (fresh randomness by design);
/// - every row id and FK reference → rank through the table's natural key
///   (insert-order differences cannot hide value divergences).
fn normalize(dump: &BTreeMap<String, (Vec<String>, Vec<Vec<String>>)>) -> String {
    // Rank maps: raw id text → `#rank` by natural-key order. Computed in
    // dependency order (the packages/releases owner cycle is broken by
    // keying packages on path, which needs no FK).
    let artists = id_ranks(dump, "artists", &|cols, row| {
        text(col(cols, row, "name")).to_lowercase()
    });
    let groups = id_ranks(dump, "release_groups", &|cols, row| {
        text(col(cols, row, "group_key"))
    });
    let packages = id_ranks(dump, "packages", &|cols, row| text(col(cols, row, "path")));
    let releases = id_ranks(dump, "releases", &|cols, row| {
        format!(
            "{}:{}",
            remap(&groups, col(cols, row, "group_id")),
            text(col(cols, row, "release_key"))
        )
    });
    let media = id_ranks(dump, "media", &|cols, row| {
        format!(
            "{}:{}",
            remap(&releases, col(cols, row, "release_id")),
            col(cols, row, "disc_number")
        )
    });
    let tracks = id_ranks(dump, "tracks", &|cols, row| {
        format!(
            "{}:{}",
            remap(&media, col(cols, row, "media_id")),
            col(cols, row, "track_number")
        )
    });
    let audio_objects = id_ranks(dump, "audio_objects", &|cols, row| {
        remap(&tracks, col(cols, row, "track_id"))
    });
    let assets = id_ranks(dump, "assets", &|cols, row| {
        format!(
            "{}:{}:{}:{}",
            remap(&releases, col(cols, row, "release_id")),
            text(col(cols, row, "kind")),
            text(col(cols, row, "role")),
            text(col(cols, row, "relative_path"))
        )
    });
    let variants = id_ranks(dump, "audio_variants", &|cols, row| {
        format!(
            "{}:{}",
            remap(&tracks, col(cols, row, "track_id")),
            text(col(cols, row, "relative_path"))
        )
    });

    let mut out = String::new();
    for (table, (cols, rows)) in dump {
        out.push_str(&format!("== {table} ==\n"));
        let mut lines: Vec<String> = rows
            .iter()
            .map(|row| {
                normalize_row(
                    table,
                    cols,
                    row,
                    &IdMaps {
                        artists: &artists,
                        groups: &groups,
                        packages: &packages,
                        releases: &releases,
                        media: &media,
                        tracks: &tracks,
                        audio_objects: &audio_objects,
                        assets: &assets,
                        variants: &variants,
                    },
                )
            })
            .collect();
        lines.sort();
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

struct IdMaps<'a> {
    artists: &'a HashMap<String, usize>,
    groups: &'a HashMap<String, usize>,
    packages: &'a HashMap<String, usize>,
    releases: &'a HashMap<String, usize>,
    media: &'a HashMap<String, usize>,
    tracks: &'a HashMap<String, usize>,
    audio_objects: &'a HashMap<String, usize>,
    assets: &'a HashMap<String, usize>,
    variants: &'a HashMap<String, usize>,
}

/// Raw cell text without the `T:` marker the dumper adds.
fn text(cell: &str) -> String {
    cell.strip_prefix("T:").unwrap_or(cell).to_string()
}

fn col<'a>(cols: &[String], row: &'a [String], name: &str) -> &'a str {
    &row[cols.iter().position(|c| c == name).unwrap()]
}

/// Rank assignment: sort rows by natural key, number deduplicated keys.
fn id_ranks(
    dump: &BTreeMap<String, (Vec<String>, Vec<Vec<String>>)>,
    table: &str,
    key: &dyn Fn(&[String], &[String]) -> String,
) -> HashMap<String, usize> {
    let empty = (Vec::new(), Vec::new());
    let (cols, rows) = dump.get(table).unwrap_or(&empty);
    let has_id = cols.iter().any(|c| c == "id");
    let mut pairs: Vec<(String, String)> = rows
        .iter()
        .map(|r| {
            (
                if has_id {
                    col(cols, r, "id").to_string()
                } else {
                    String::new()
                },
                key(cols, r),
            )
        })
        .collect();
    pairs.sort_by(|a, b| a.1.cmp(&b.1));
    let mut map = HashMap::new();
    let mut last: Option<&str> = None;
    for (rank, (id, k)) in pairs.iter().enumerate() {
        if last != Some(k.as_str()) {
            last = Some(k);
        }
        // First id wins a shared rank; duplicate natural keys cannot
        // occur for the keyed tables (and joins carry no id map).
        map.entry(id.clone()).or_insert(rank);
    }
    map
}

fn remap(rank: &HashMap<String, usize>, raw_id: &str) -> String {
    match rank.get(raw_id) {
        Some(i) => format!("#{i}"),
        None => format!("?{raw_id}"),
    }
}

// ---- comparison driver + scenarios ---------------------------------------

fn c_binary() -> Option<PathBuf> {
    std::env::var_os("MUSICPACK_LEGACY_SERVER").map(PathBuf::from)
}

/// Compares two normalized dumps; on mismatch prints a unified diff-style
/// listing of the first diverging lines.
fn assert_same_db(c: &str, r: &str, context: &str) {
    if c == r {
        return;
    }
    let c_lines: Vec<&str> = c.lines().collect();
    let r_lines: Vec<&str> = r.lines().collect();
    let mut report = format!("database divergence after {context}:\n");
    for (i, (a, b)) in c_lines.iter().zip(r_lines.iter()).enumerate() {
        if a != b {
            report.push_str(&format!("line {i}:\n  C:    {a}\n  Rust: {b}\n"));
            if report.len() > 4000 {
                report.push_str("  ... (truncated)\n");
                break;
            }
        }
    }
    if c_lines.len() != r_lines.len() {
        report.push_str(&format!(
            "line counts differ: C={} Rust={}\n",
            c_lines.len(),
            r_lines.len()
        ));
    }
    panic!("{report}");
}

/// Scans both libraries (C when available, Rust always), compares counters
/// and normalized databases, and returns the Rust result.
#[allow(clippy::too_many_arguments)]
fn parity_round(
    c_bin: &Option<PathBuf>,
    lib_c: &Path,
    lib_r: &Path,
    db_c: &Path,
    db_r: &Path,
    verify: bool,
    context: &str,
) -> ScanResult {
    let rust = if verify {
        scan_rust_verify(lib_r, db_r)
    } else {
        scan_rust(lib_r, db_r)
    };
    let Some(bin) = c_bin else {
        eprintln!("notice: MUSICPACK_LEGACY_SERVER is not set; C comparison skipped ({context})");
        return rust;
    };
    let c_summary = if verify {
        wait_for_next_second();
        let output = Command::new(bin)
            .arg("verify")
            .arg("--library")
            .arg(lib_c)
            .arg("--database")
            .arg(db_c)
            .env_remove("MUSICPACK_LOG")
            .output()
            .expect("failed to run the legacy server binary");
        assert!(output.status.success());
        parse_c_summary(&String::from_utf8_lossy(&output.stdout))
    } else {
        scan_c(bin, lib_c, db_c)
    };
    assert_eq!(
        CSummary {
            total: rust.total,
            added: rust.added,
            updated: rust.updated,
            moved: rust.moved,
            removed: rust.removed,
            invalid: rust.invalid,
        },
        c_summary,
        "scan counters diverge ({context})"
    );
    let norm_c = normalize(&dump_db(db_c));
    let norm_r = normalize(&dump_db(db_r));
    assert_same_db(&norm_c, &norm_r, context);
    rust
}

/// Builds the same library twice (one per implementation) using `build`.
fn mirrored_libs(root: &Path, build: &dyn Fn(&Path)) -> (Side, Side) {
    let c = fresh_side(root, "c");
    let r = fresh_side(root, "r");
    build(&c.lib);
    build(&r.lib);
    (c, r)
}

/// Applies `mutate` to both libraries.
fn mutate_both(c: &Side, r: &Side, mutate: &dyn Fn(&Path)) {
    mutate(&c.lib);
    mutate(&r.lib);
}

fn package_status(db: &Path, name: &str) -> (String, String) {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(
        "SELECT status, verify_status FROM packages WHERE path LIKE ?1",
        rusqlite::params![format!("%{name}")],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )
    .unwrap()
}

fn table_count(db: &Path, table: &str) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

#[test]
fn basic_ingest_matches_with_stable_absolute_ids() {
    let root = test_root("basic");
    let c_bin = c_binary();
    let (c, r) = mirrored_libs(&root, &|lib| {
        build_basic(lib, "album.mpack");
    });
    let result = parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, false, "basic ingest");
    assert_eq!(
        result,
        ScanResult {
            total: 1,
            added: 1,
            updated: 0,
            moved: 0,
            removed: 0,
            invalid: 0,
            cancelled: false,
        }
    );
    // Absolute ids on a single-package library (no ordering ambiguity).
    let conn = rusqlite::Connection::open(&r.db).unwrap();
    let one = |sql: &str| conn.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();
    assert_eq!(one("SELECT id FROM packages"), 1);
    assert_eq!(one("SELECT id FROM release_groups"), 1);
    assert_eq!(one("SELECT id FROM releases"), 1);
    assert_eq!(one("SELECT owner_package_id FROM releases"), 1);
    assert_eq!(one("SELECT COUNT(*) FROM media"), 2);
    assert_eq!(one("SELECT COUNT(*) FROM tracks"), 3);
    assert_eq!(one("SELECT COUNT(*) FROM audio_objects"), 3);
    assert_eq!(one("SELECT COUNT(*) FROM assets"), 4);
    assert_eq!(one("SELECT COUNT(*) FROM track_waveforms"), 2);
    assert_eq!(one("SELECT COUNT(*) FROM audio_variants"), 1);
    assert_eq!(one("SELECT COUNT(*) FROM artists"), 2);
    // Probe facts landed: Musepack SV8 + FLAC with real stream facts.
    let (codec, version, rate, channels): (String, i64, i64, i64) = conn
        .query_row(
            "SELECT codec, stream_version, sample_rate, channels FROM audio_objects
             WHERE relative_path = 'audio/01.mpc'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(codec, "musepack-sv8");
    assert_eq!((version, rate, channels), (8, 44100, 2));
    let (codec, rate, channels): (String, i64, i64) = conn
        .query_row(
            "SELECT codec, sample_rate, channels FROM audio_objects
             WHERE relative_path = 'audio/02.flac'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(codec, "flac");
    assert_eq!((rate, channels), (44100, 2));
    // Placeholder bytes fail both probes → extension codec, zeroed numbers.
    let codec: String = conn
        .query_row(
            "SELECT codec FROM audio_objects WHERE relative_path = 'audio/03.wav'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(codec, "wav");
    assert_eq!(
        package_status(&r.db, "album.mpack"),
        ("valid".into(), "unverified".into())
    );
}

#[test]
fn rescan_is_a_stable_no_op() {
    let root = test_root("stable");
    let c_bin = c_binary();
    let (c, r) = mirrored_libs(&root, &|lib| {
        build_basic(lib, "album.mpack");
    });
    parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, false, "round 1");
    let before = normalize(&dump_db(&r.db));
    let result = parity_round(
        &c_bin,
        &c.lib,
        &r.lib,
        &c.db,
        &r.db,
        false,
        "round 2 unchanged",
    );
    assert_eq!(
        result,
        ScanResult {
            total: 1,
            added: 0,
            updated: 0,
            moved: 0,
            removed: 0,
            invalid: 0,
            cancelled: false,
        }
    );
    // Normalized state is bit-identical across the rescan (timestamps and
    // uids redacted; everything else must not move).
    assert_eq!(normalize(&dump_db(&r.db)), before);
}

#[test]
fn title_only_change_keeps_all_row_ids() {
    let root = test_root("retitle");
    let c_bin = c_binary();
    let (c, r) = mirrored_libs(&root, &|lib| {
        build_basic(lib, "album.mpack");
    });
    parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, false, "round 1");
    let rename = |lib: &Path| {
        let manifest = lib.join("album.mpack/manifest.json");
        let text = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(
            &manifest,
            text.replacen("\"title\":\"First\"", "\"title\":\"First (Remastered)\"", 1),
        )
        .unwrap();
    };
    mutate_both(&c, &r, &rename);
    let result = parity_round(
        &c_bin,
        &c.lib,
        &r.lib,
        &c.db,
        &r.db,
        false,
        "round 2 retitle",
    );
    assert_eq!(result.updated, 1);
    // Track/media/package ids are stable; only the title changed.
    let conn = rusqlite::Connection::open(&r.db).unwrap();
    let title: String = conn
        .query_row("SELECT title FROM tracks WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(title, "First (Remastered)");
    assert_eq!(table_count(&r.db, "tracks"), 3);
}

#[test]
fn audio_change_replaces_the_track_row() {
    let root = test_root("newaudio");
    let c_bin = c_binary();
    let (c, r) = mirrored_libs(&root, &|lib| {
        build_basic(lib, "album.mpack");
    });
    parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, false, "round 1");
    let swap_audio = |lib: &Path| {
        let dir = lib.join("album.mpack");
        let sha = write_file(&dir, "audio/01.mpc", b"completely-different-bytes");
        // Patch the manifest's audio sha for track 1 (the audio/01.mpc
        // object; single-line JSON by construction).
        let manifest = dir.join("manifest.json");
        let text = std::fs::read_to_string(&manifest).unwrap();
        let marker = "\"path\":\"audio/01.mpc\",\"sha256\":\"";
        let pos = text.find(marker).unwrap() + marker.len();
        let mut patched = text;
        patched.replace_range(pos..pos + 64, &sha);
        std::fs::write(&manifest, patched).unwrap();
    };
    mutate_both(&c, &r, &swap_audio);
    let result = parity_round(
        &c_bin,
        &c.lib,
        &r.lib,
        &c.db,
        &r.db,
        false,
        "round 2 new audio",
    );
    assert_eq!(result.updated, 1);
    // The changed track got a new row; survivors kept their ids.
    let conn = rusqlite::Connection::open(&r.db).unwrap();
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM tracks ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(ids, vec![2, 3, 4], "track 1 replaced, survivors stable");
}

#[test]
fn track_removal_matches_survivors_by_content() {
    let root = test_root("rmtrack");
    let c_bin = c_binary();
    let (c, r) = mirrored_libs(&root, &|lib| {
        build_basic(lib, "album.mpack");
    });
    parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, false, "round 1");
    // Drop track 2, renumber track 3 → 2 with identical bytes: the
    // survivor must keep its row id via the content fallback.
    let drop_middle = |lib: &Path| {
        let manifest = lib.join("album.mpack/manifest.json");
        let mut text = std::fs::read_to_string(&manifest).unwrap();
        // Track 2's object runs from the comma preceding `{"track":2`
        // to its waveform close (`"points":20` is unique to track 2);
        // removing both leaves a valid single-track array.
        let t2 = text.find("{\"track\":2").unwrap();
        let end_marker = "\"points\":20}}";
        let end = text.find(end_marker).unwrap() + end_marker.len();
        text.replace_range(t2 - 1..end, "");
        // Renumber disc 2's track 1 → 2 (same audio bytes).
        text = text.replacen(
            "\"track\":1,\"title\":\"Third\"",
            "\"track\":2,\"title\":\"Third\"",
            1,
        );
        std::fs::write(&manifest, text).unwrap();
    };
    mutate_both(&c, &r, &drop_middle);
    parity_round(
        &c_bin,
        &c.lib,
        &r.lib,
        &c.db,
        &r.db,
        false,
        "round 2 drop track",
    );
    let conn = rusqlite::Connection::open(&r.db).unwrap();
    let numbers: Vec<(i64, i64)> = conn
        .prepare("SELECT id, track_number FROM tracks ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    // Track id 3 survived with its new number; track id 2 is gone.
    assert!(numbers.contains(&(1, 1)));
    assert!(numbers.contains(&(3, 2)));
    assert_eq!(numbers.len(), 2);
}

#[test]
fn remove_and_reappear_cycles_through_unavailable() {
    let root = test_root("fairy");
    let c_bin = c_binary();
    let (c, r) = mirrored_libs(&root, &|lib| {
        build_basic(lib, "album.mpack");
    });
    parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, false, "round 1");
    // Round 2: the package vanishes → sweep marks it unavailable.
    mutate_both(&c, &r, &|lib| {
        std::fs::remove_dir_all(lib.join("album.mpack")).unwrap();
    });
    let result = parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, false, "round 2 gone");
    assert_eq!(
        result,
        ScanResult {
            total: 0,
            added: 0,
            updated: 0,
            moved: 0,
            removed: 1,
            invalid: 0,
            cancelled: false,
        }
    );
    assert_eq!(
        package_status(&r.db, "album.mpack"),
        ("unavailable".into(), "unverified".into())
    );
    // Round 3: byte-identical package reappears → valid via the fast path.
    mutate_both(&c, &r, &|lib| {
        build_basic(lib, "album.mpack");
    });
    let result = parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, false, "round 3 back");
    assert_eq!(result.total, 1);
    assert_eq!(
        package_status(&r.db, "album.mpack"),
        ("valid".into(), "unverified".into())
    );
}

#[test]
fn invalid_lifecycle_records_and_recovers() {
    let root = test_root("invalid");
    let c_bin = c_binary();
    let build_broken = |lib: &Path| {
        let dir = lib.join("broken.mpack");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), b"{not json").unwrap();
    };
    let (c, r) = mirrored_libs(&root, &build_broken);
    let result = parity_round(
        &c_bin,
        &c.lib,
        &r.lib,
        &c.db,
        &r.db,
        false,
        "round 1 broken",
    );
    assert_eq!(result.invalid, 1);
    assert_eq!(
        package_status(&r.db, "broken.mpack"),
        ("invalid".into(), "unverified".into())
    );
    // Unchanged breakage: total only (invalid-sticky fast path).
    let result = parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, false, "round 2 same");
    assert_eq!(
        result,
        ScanResult {
            total: 1,
            added: 0,
            updated: 0,
            moved: 0,
            removed: 0,
            invalid: 0,
            cancelled: false,
        }
    );
    // Fixed manifest ingests normally.
    mutate_both(&c, &r, &|lib| {
        build_minimal(lib, "broken.mpack", "Fixed", "Fixer", "");
    });
    let result = parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, false, "round 3 fixed");
    assert_eq!(result.updated, 1);
    assert_eq!(
        package_status(&r.db, "broken.mpack"),
        ("valid".into(), "unverified".into())
    );
}

#[test]
fn mirror_pair_shares_one_owner_then_transfers() {
    let root = test_root("mirror");
    let c_bin = c_binary();
    // Two byte-identical packages (same fingerprint, different paths).
    let build_pair = |lib: &Path| {
        build_minimal(lib, "a-first.mpack", "Mirror", "M", "");
        let src = lib.join("a-first.mpack/manifest.json");
        let dst_dir = lib.join("b-second.mpack");
        std::fs::create_dir_all(dst_dir.join("audio")).unwrap();
        std::fs::copy(src, dst_dir.join("manifest.json")).unwrap();
        std::fs::copy(
            lib.join("a-first.mpack/audio/01.mpc"),
            dst_dir.join("audio/01.mpc"),
        )
        .unwrap();
    };
    let (c, r) = mirrored_libs(&root, &build_pair);
    let result = parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, false, "round 1 pair");
    assert_eq!(result.added, 2);
    // First-discovered-wins arbitration, verified per side without
    // assuming discovery order: package rows are inserted in discovery
    // order, so the owner must hold the lowest package id of the release.
    // The C side is only checked when it actually ran.
    let mut sides = vec![&r.db];
    if c_bin.is_some() {
        sides.push(&c.db);
    }
    for db in sides {
        let conn = rusqlite::Connection::open(db).unwrap();
        let owner: i64 = conn
            .query_row("SELECT owner_package_id FROM releases", [], |row| {
                row.get(0)
            })
            .unwrap();
        let first: i64 = conn
            .query_row(
                "SELECT MIN(p.id) FROM packages p JOIN releases r ON r.id = p.release_id",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owner, first, "first discovered must own ({db:?})");
    }
    // Remove the owner: the mirror stays valid but its owner is
    // unavailable (the fast path does not re-arbitrate).
    mutate_both(&c, &r, &|lib| {
        std::fs::remove_dir_all(lib.join("a-first.mpack")).unwrap();
    });
    parity_round(
        &c_bin,
        &c.lib,
        &r.lib,
        &c.db,
        &r.db,
        false,
        "round 2 owner gone",
    );
    assert_eq!(
        package_status(&r.db, "b-second.mpack"),
        ("valid".into(), "unverified".into())
    );
    // Touch the survivor's manifest (whitespace changes the fingerprint):
    // full ingest re-arbitrates and it takes ownership.
    mutate_both(&c, &r, &|lib| {
        let manifest = lib.join("b-second.mpack/manifest.json");
        let mut text = std::fs::read_to_string(&manifest).unwrap();
        text.push(' ');
        std::fs::write(&manifest, text).unwrap();
    });
    parity_round(
        &c_bin,
        &c.lib,
        &r.lib,
        &c.db,
        &r.db,
        false,
        "round 3 takeover",
    );
    // Single surviving candidate: the takeover target is deterministic on
    // both sides (checked only where that side ran).
    let mut sides = vec![&r.db];
    if c_bin.is_some() {
        sides.push(&c.db);
    }
    for db in sides {
        let conn = rusqlite::Connection::open(db).unwrap();
        let second: i64 = conn
            .query_row(
                "SELECT id FROM packages WHERE path LIKE '%b-second.mpack'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let owner: i64 = conn
            .query_row("SELECT owner_package_id FROM releases", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(owner, second, "survivor must own ({db:?})");
    }
}

#[test]
fn conflict_quarantine_holds_then_releases() {
    let root = test_root("conflict");
    let c_bin = c_binary();
    // Same identity (titles/artists/edition), different manifest bytes
    // (different track title → different fingerprint): a genuine conflict,
    // not a mirror. Packages arrive in separate rounds so the outcome is
    // deterministic regardless of discovery order.
    let build_owner = |lib: &Path| {
        build_minimal(
            lib,
            "a-owner.mpack",
            "Rival",
            "R",
            r#","release":{"edition":"One"}"#,
        );
    };
    let add_rival = |lib: &Path| {
        build_minimal(
            lib,
            "b-rival.mpack",
            "Rival",
            "R",
            r#","release":{"edition":"One"}"#,
        );
        let manifest = lib.join("b-rival.mpack/manifest.json");
        let text = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(
            &manifest,
            text.replacen("\"title\":\"T\"", "\"title\":\"T2\"", 1),
        )
        .unwrap();
    };
    let (c, r) = mirrored_libs(&root, &build_owner);
    parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, false, "round 1 owner");
    assert_eq!(
        package_status(&r.db, "a-owner.mpack"),
        ("valid".into(), "unverified".into())
    );
    // Round 2: the rival arrives after the owner exists → quarantined.
    mutate_both(&c, &r, &add_rival);
    parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, false, "round 2 rival");
    assert_eq!(
        package_status(&r.db, "b-rival.mpack"),
        ("conflict".into(), "unverified".into())
    );
    // Repeated scans keep the quarantine without touching the owner.
    parity_round(
        &c_bin,
        &c.lib,
        &r.lib,
        &c.db,
        &r.db,
        false,
        "round 3 repeat",
    );
    assert_eq!(
        package_status(&r.db, "b-rival.mpack"),
        ("conflict".into(), "unverified".into())
    );
    // Remove the owner: the rival is unchanged, so the fast path applies
    // and does NOT re-arbitrate — it stays quarantined (oracle law).
    mutate_both(&c, &r, &|lib| {
        std::fs::remove_dir_all(lib.join("a-owner.mpack")).unwrap();
    });
    parity_round(
        &c_bin,
        &c.lib,
        &r.lib,
        &c.db,
        &r.db,
        false,
        "round 4 owner gone",
    );
}

#[test]
fn warning_for_missing_audio_sticks_after_repair() {
    let root = test_root("warning");
    let c_bin = c_binary();
    let (c, r) = mirrored_libs(&root, &|lib| {
        build_minimal(lib, "thin.mpack", "Thin", "T", "");
    });
    // Delete the audio after building: light scan reports warning.
    mutate_both(&c, &r, &|lib| {
        std::fs::remove_file(lib.join("thin.mpack/audio/01.mpc")).unwrap();
    });
    let result = parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, false, "round 1 thin");
    assert_eq!(result.added, 1);
    assert_eq!(
        package_status(&r.db, "thin.mpack"),
        ("warning".into(), "unverified".into())
    );
    // Restore the file: same sha → fast path finds nothing missing, but
    // the row keeps its warning status (only unavailable rows flip back).
    mutate_both(&c, &r, &|lib| {
        std::fs::write(lib.join("thin.mpack/audio/01.mpc"), b"placeholder").unwrap();
    });
    parity_round(
        &c_bin,
        &c.lib,
        &r.lib,
        &c.db,
        &r.db,
        false,
        "round 2 repaired",
    );
    assert_eq!(
        package_status(&r.db, "thin.mpack"),
        ("warning".into(), "unverified".into())
    );
}

#[test]
fn moved_package_keeps_its_row() {
    let root = test_root("moved");
    let c_bin = c_binary();
    let (c, r) = mirrored_libs(&root, &|lib| {
        build_minimal(lib, "old.mpack", "Mover", "M", "");
    });
    parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, false, "round 1");
    // Rename the directory (same bytes): the row moves, identity stays.
    mutate_both(&c, &r, &|lib| {
        std::fs::rename(lib.join("old.mpack"), lib.join("new.mpack")).unwrap();
    });
    let result = parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, false, "round 2 moved");
    assert_eq!(result.moved, 1);
    let conn = rusqlite::Connection::open(&r.db).unwrap();
    let (id, path): (i64, String) = conn
        .query_row("SELECT id, path FROM packages", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(id, 1);
    assert!(path.ends_with("new.mpack"));
}

#[test]
fn verify_mode_marks_fully_verified_packages() {
    let root = test_root("verify");
    let c_bin = c_binary();
    let (c, r) = mirrored_libs(&root, &|lib| {
        build_basic(lib, "album.mpack");
    });
    let result = parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, true, "verify round");
    assert_eq!(result.added, 1);
    // The basic fixture declares duration 180.5 s with only 10 waveform
    // buckets: both implementations warn on the duration/points divergence
    // (the oracle above proves they agree exactly).
    assert_eq!(
        package_status(&r.db, "album.mpack"),
        ("warning".into(), "warning".into())
    );
}

#[test]
fn musicbrainz_grouping_shares_one_group() {
    let root = test_root("mbgroup");
    let c_bin = c_binary();
    let build_editions = |lib: &Path| {
        let group = r#","identifiers":{"musicbrainzReleaseGroupId":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"}"#;
        build_minimal(
            lib,
            "ed1.mpack",
            "Editions",
            "E",
            &format!(r#"{group},"release":{{"edition":"First"}}"#),
        );
        build_minimal(
            lib,
            "ed2.mpack",
            "Editions",
            "E",
            &format!(r#"{group},"release":{{"edition":"Second"}}"#),
        );
    };
    let (c, r) = mirrored_libs(&root, &build_editions);
    parity_round(&c_bin, &c.lib, &r.lib, &c.db, &r.db, false, "two editions");
    let conn = rusqlite::Connection::open(&r.db).unwrap();
    let groups: i64 = conn
        .query_row("SELECT COUNT(*) FROM release_groups", [], |row| row.get(0))
        .unwrap();
    let releases: i64 = conn
        .query_row("SELECT COUNT(*) FROM releases", [], |row| row.get(0))
        .unwrap();
    assert_eq!((groups, releases), (1, 2));
}

fn is_timestamp(cell: &str) -> bool {
    let t = text(cell);
    t.len() == 19
        && t.as_bytes().iter().enumerate().all(|(i, &b)| match i {
            4 | 7 => b == b'-',
            10 => b == b' ',
            13 | 16 => b == b':',
            _ => b.is_ascii_digit(),
        })
}

fn normalize_row(table: &str, cols: &[String], row: &[String], maps: &IdMaps<'_>) -> String {
    let rewrite_id = |map: &HashMap<String, usize>, cell: &str| {
        if cell == "NULL" {
            return "NULL".to_string();
        }
        remap(map, cell)
    };
    cols.iter()
        .zip(row.iter())
        .map(|(name, cell)| {
            let value = match (table, name.as_str()) {
                (_, "created_at") | (_, "updated_at") | (_, "applied_at") => {
                    assert!(
                        is_timestamp(cell),
                        "{table}.{name} has no timestamp shape: {cell}"
                    );
                    "<TS>".to_string()
                }
                (_, "last_scan") => {
                    let t = text(cell);
                    assert!(
                        t.starts_with('s') && t.contains('.'),
                        "{table}.{name} has no scan-token shape: {cell}"
                    );
                    "<SCAN>".to_string()
                }
                (_, "uid") => {
                    let t = text(cell);
                    assert!(
                        t.len() == 32 && t.bytes().all(|b| b.is_ascii_hexdigit()),
                        "{table}.{name} has no uid shape: {cell}"
                    );
                    "<UID>".to_string()
                }
                // v11 comparison boundary: each side records its own
                // latest schema version (the C tops out at 10, the Rust
                // server at 11 — additive migration, same state).
                ("schema_version", "version") => {
                    assert!(
                        cell == "10" || cell == "11",
                        "{table}.{name} is neither implementation's latest: {cell}"
                    );
                    "<VER>".to_string()
                }
                ("artists", "id") => rewrite_id(maps.artists, cell),
                ("release_groups", "id") => rewrite_id(maps.groups, cell),
                ("packages", "id") => rewrite_id(maps.packages, cell),
                ("releases", "id") => rewrite_id(maps.releases, cell),
                ("media", "id") => rewrite_id(maps.media, cell),
                ("tracks", "id") => rewrite_id(maps.tracks, cell),
                ("audio_objects", "id") => rewrite_id(maps.audio_objects, cell),
                ("assets", "id") => rewrite_id(maps.assets, cell),
                ("audio_variants", "id") => rewrite_id(maps.variants, cell),
                (_, "group_id") => rewrite_id(maps.groups, cell),
                (_, "release_id") => rewrite_id(maps.releases, cell),
                (_, "media_id") => rewrite_id(maps.media, cell),
                (_, "track_id") => rewrite_id(maps.tracks, cell),
                (_, "artist_id") => rewrite_id(maps.artists, cell),
                (_, "owner_package_id") => {
                    // Redacted by shape: owner arbitration is
                    // first-discovered-wins, and discovery order is
                    // implementation-specific (C: readdir order, Rust:
                    // sorted paths). The arbitration RULE is asserted
                    // structurally per side (owner == lowest package id
                    // of the release); the absolute choice cannot be
                    // compared across implementations when several
                    // candidates race. Single-candidate takeovers are
                    // deterministic and asserted explicitly per side.
                    if cell != "NULL" {
                        cell.parse::<i64>().expect("owner must be numeric");
                    }
                    "<OWNER>".to_string()
                }
                ("packages", "path") => {
                    // Oracle libraries are flat (every package is a direct
                    // child of the walk root); the basename is the stable
                    // identity. The absolute scratch prefix (including the
                    // per-implementation lib-c/lib-r split) is
                    // construction noise.
                    let t = text(cell);
                    format!("T:{}", t.rsplit('/').next().unwrap_or(&t))
                }
                _ => cell.clone(),
            };
            format!("{name}={value}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}
