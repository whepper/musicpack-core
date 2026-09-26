use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::Result;

#[derive(Debug, Clone)]
pub struct TrackSpec {
    pub path: PathBuf,
    pub artist: String,
    pub album: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct Corpus {
    pub root: PathBuf,
    pub albums: Vec<Vec<TrackSpec>>,
    pub discovered_files: usize,
    pub unsupported_audio_files: usize,
}

#[derive(Debug, Clone)]
pub struct CrossCodecPair {
    pub flac: PathBuf,
    pub mpc: PathBuf,
}

/// Finds likely equivalent FLAC/Musepack candidates by matching the scanner's
/// artist/album proxy and filename stem. This is deliberately a candidate
/// finder, not an assertion that the files are the same recording.
pub fn find_cross_codec_pairs(root: &Path, limit: usize) -> Result<(usize, Vec<CrossCodecPair>)> {
    let mut files = Vec::new();
    let mut unsupported = 0usize;
    collect(root, &mut files, &mut unsupported)?;
    files.sort();

    let mut grouped: BTreeMap<(String, String, String), BTreeMap<String, PathBuf>> =
        BTreeMap::new();
    for path in files {
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        let extension = extension.to_ascii_lowercase();
        if extension != "flac" && extension != "mpc" {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let spec = track_spec(root, &path);
        grouped
            .entry((
                spec.artist.to_ascii_lowercase(),
                spec.album.to_ascii_lowercase(),
                stem.to_ascii_lowercase(),
            ))
            .or_default()
            .insert(extension, path);
    }

    let mut all_pairs = grouped
        .into_values()
        .filter_map(|mut formats| {
            let flac = formats.remove("flac")?;
            let mpc = formats.remove("mpc")?;
            Some(CrossCodecPair { flac, mpc })
        })
        .collect::<Vec<_>>();
    let total = all_pairs.len();
    all_pairs.truncate(limit);
    Ok((total, all_pairs))
}

pub fn select_library(root: &Path, album_limit: usize, track_limit: usize) -> Result<Corpus> {
    let mut files = Vec::new();
    let mut unsupported = 0usize;
    collect(root, &mut files, &mut unsupported)?;
    files.sort();
    let discovered_files = files.len();

    let mut groups: BTreeMap<(String, String), Vec<TrackSpec>> = BTreeMap::new();
    for path in files {
        let spec = track_spec(root, &path);
        groups
            .entry((spec.artist.clone(), spec.album.clone()))
            .or_default()
            .push(spec);
    }

    let all_albums: Vec<Vec<TrackSpec>> = groups
        .into_values()
        .map(|tracks| evenly_spaced(tracks, track_limit))
        .collect();
    let selected = evenly_spaced(all_albums, album_limit);

    Ok(Corpus {
        root: root.to_path_buf(),
        albums: selected,
        discovered_files,
        unsupported_audio_files: unsupported,
    })
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>, unsupported: &mut usize) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'))
            {
                continue;
            }
            collect(&path, files, unsupported)?;
        } else if file_type.is_file() {
            match path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("flac" | "wav" | "mpc") => files.push(path),
                Some("mp3" | "m4a" | "ogg") => *unsupported += 1,
                _ => {}
            }
        }
    }
    Ok(())
}

fn track_spec(root: &Path, path: &Path) -> TrackSpec {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let mut components = relative.components();
    let file_name = components
        .next()
        .and_then(|component| component.as_os_str().to_str())
        .unwrap_or("track")
        .to_string();
    let title = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(&file_name)
        .to_string();
    let parent = path.parent();
    let album = parent
        .and_then(|p| p.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("unknown-album")
        .to_string();
    let artist = parent
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("unknown-artist")
        .to_string();
    TrackSpec {
        path: path.to_path_buf(),
        artist,
        album,
        title,
    }
}

fn evenly_spaced<T>(values: Vec<T>, limit: usize) -> Vec<T> {
    if values.len() <= limit {
        return values;
    }
    let value_count = values.len();
    let mut selected = Vec::with_capacity(limit);
    let mut iterator = values.into_iter();
    let mut previous = 0usize;
    for index in 0..limit {
        let source_index = index * value_count / limit;
        for _ in previous..source_index {
            iterator.next();
        }
        selected.push(iterator.next().unwrap());
        previous = source_index + 1;
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::evenly_spaced;

    #[test]
    fn evenly_spaced_selection_is_stable() {
        let values = (0..10).collect::<Vec<_>>();
        assert_eq!(evenly_spaced(values, 3), vec![0, 3, 6]);
    }
}
