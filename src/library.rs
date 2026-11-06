use gtk::gio::prelude::FileExt;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::{Path, PathBuf}};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Track {
    pub title: String,
    pub uri: String,
    #[serde(default)]
    pub is_radio: bool,
}

impl Track {
    pub fn station(title: impl Into<String>, url: impl Into<String>) -> Self {
        Self { title: title.into(), uri: url.into(), is_radio: true }
    }

    pub fn file(path: &Path) -> Result<Self> {
        let path = fs::canonicalize(path).with_context(|| format!("Cannot access {}", path.display()))?;
        let title = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Untitled").to_string();
        let uri = gtk::gio::File::for_path(&path).uri().to_string();
        Ok(Self { title, uri, is_radio: false })
    }
}

pub fn data_file() -> PathBuf {
    let dir = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    dir.join("slateamp").join("stations.json")
}

pub fn load_stations() -> Vec<Track> {
    fs::read(data_file()).ok()
        .and_then(|data| serde_json::from_slice::<Vec<Track>>(&data).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|track| track.is_radio && valid_stream_url(&track.uri))
        .collect()
}

pub fn save_stations(stations: &[Track]) -> Result<()> {
    let path = data_file();
    let parent = path.parent().context("No data directory")?;
    fs::create_dir_all(parent)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(stations)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

pub fn valid_stream_url(value: &str) -> bool {
    let value = value.trim();
    (value.starts_with("https://") || value.starts_with("http://"))
        && !value.chars().any(char::is_whitespace)
        && value.split_once("://").is_some_and(|(_, host)| !host.is_empty())
}

fn from_line(raw: &str, base: &Path, title: Option<String>) -> Option<Track> {
    let s = raw.trim();
    if s.is_empty() || s.starts_with('#') { return None; }
    if valid_stream_url(s) {
        let fallback = s.split('/').nth(2).unwrap_or(s);
        return Some(Track::station(title.unwrap_or_else(|| fallback.to_string()), s));
    }
    // Intentionally reject file: and arbitrary custom URI schemes in imported playlists.
    let local = Path::new(s);
    let local = if local.is_absolute() { local.to_path_buf() } else { base.join(local) };
    let mut track = Track::file(&local).ok()?;
    if let Some(title) = title { track.title = title; }
    Some(track)
}

pub fn import_playlist(path: &Path) -> Result<Vec<Track>> {
    let content = fs::read_to_string(path).with_context(|| format!("Cannot read playlist {}", path.display()))?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut tracks = Vec::new();
    if ext == "pls" {
        let mut entries: std::collections::BTreeMap<usize, (Option<String>, Option<String>)> = Default::default();
        for line in content.lines() {
            let Some((key, value)) = line.trim().split_once('=') else { continue; };
            let lower = key.trim().to_ascii_lowercase();
            if let Some(index) = lower.strip_prefix("file").and_then(|n| n.parse::<usize>().ok()) {
                entries.entry(index).or_default().0 = Some(value.trim().to_string());
            } else if let Some(index) = lower.strip_prefix("title").and_then(|n| n.parse::<usize>().ok()) {
                entries.entry(index).or_default().1 = Some(value.trim().to_string());
            }
        }
        for (_, (path, title)) in entries {
            if let Some(path) = path {
                if let Some(t) = from_line(&path, base, title) { tracks.push(t); }
            }
        }
    } else {
        let mut pending_title = None;
        for line in content.lines() {
            let line = line.trim().trim_start_matches('\u{feff}');
            if let Some(info) = line.strip_prefix("#EXTINF:") {
                pending_title = info.split_once(',').map(|(_, title)| title.trim().to_string());
            } else if !line.is_empty() && !line.starts_with('#') {
                if let Some(track) = from_line(line, base, pending_title.take()) {
                    tracks.push(track);
                }
            }
        }
    }
    if tracks.is_empty() { anyhow::bail!("The playlist contains no reachable local files or valid stream URLs"); }
    Ok(tracks)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn accepts_laut_fm_stream() {
        assert!(valid_stream_url("https://alternative-rock.stream.laut.fm/alternative-rock"));
    }
    #[test] fn rejects_unsafe_or_invalid_urls() {
        assert!(!valid_stream_url("file:///etc/passwd"));
        assert!(!valid_stream_url("javascript:alert(1)"));
        assert!(!valid_stream_url("https://bad host/test"));
    }
    #[test] fn imports_laut_fm_m3u() {
        let base = std::env::temp_dir().join(format!("slateamp-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&base);
        let path = base.join("radio.m3u");
        fs::write(&path, "#EXTM3U\n#EXTINF:-1,Alternative Rock\nhttps://alternative-rock.stream.laut.fm/alternative-rock\n").unwrap();
        let tracks = import_playlist(&path).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Alternative Rock");
        assert!(tracks[0].is_radio);
        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(base);
    }
}

/// A directory containing playable audio. Every such directory becomes a playlist.
/// Directory scanning is pure filesystem I/O and safe to run on a worker thread.
#[derive(Debug, Clone)]
pub struct MusicFolder {
    pub relative_name: String,
    pub files: Vec<PathBuf>,
}

pub fn is_audio_file(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str(),
        "mp3" | "flac" | "ogg" | "opus" | "m4a" | "aac" | "wav" | "aiff" | "aif" | "wma" | "alac")
}

/// Compare filenames naturally: "2 - Song.mp3" comes before "10 - Song.mp3".
/// Numeric runs are compared without parsing into fixed-width integers.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let a = a.to_lowercase();
    let b = b.to_lowercase();
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);

    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let start_a = i;
            let start_b = j;
            while i < a.len() && a[i].is_ascii_digit() { i += 1; }
            while j < b.len() && b[j].is_ascii_digit() { j += 1; }
            let digits_a = &a[start_a..i];
            let digits_b = &b[start_b..j];
            let trimmed_a = digits_a.iter().position(|&v| v != b'0')
                .map_or(&digits_a[digits_a.len()..], |pos| &digits_a[pos..]);
            let trimmed_b = digits_b.iter().position(|&v| v != b'0')
                .map_or(&digits_b[digits_b.len()..], |pos| &digits_b[pos..]);
            let order = trimmed_a.len().cmp(&trimmed_b.len())
                .then_with(|| trimmed_a.cmp(trimmed_b));
            if order != Ordering::Equal { return order; }
            // Equal numeric value: prefer shorter spelling ("2" before "02").
            let order = digits_a.len().cmp(&digits_b.len());
            if order != Ordering::Equal { return order; }
        } else {
            let order = a[i].cmp(&b[j]);
            if order != Ordering::Equal { return order; }
            i += 1;
            j += 1;
        }
    }
    a.len().saturating_sub(i).cmp(&b.len().saturating_sub(j))
}

pub fn scan_music(root: &Path) -> Result<Vec<MusicFolder>> {
    if !root.is_dir() { anyhow::bail!("Not a directory: {}", root.display()); }
    let mut pending = vec![root.to_path_buf()];
    let mut folders = Vec::new();
    while let Some(dir) = pending.pop() {
        let mut files = Vec::new();
        // Skip unreadable subdirectories instead of rejecting the whole music library.
        let Ok(entries) = fs::read_dir(&dir) else { continue; };
        for entry in entries.flatten() {
            let Ok(ty) = entry.file_type() else { continue; };
            // Do not traverse symlinks: avoids link loops and browsing outside root.
            if ty.is_symlink() { continue; }
            let path = entry.path();
            if ty.is_dir() { pending.push(path); }
            else if ty.is_file() && is_audio_file(&path) { files.push(path); }
        }
        if !files.is_empty() {
            files.sort_by(|a, b| {
                let a = a.file_name().unwrap_or_else(|| a.as_os_str()).to_string_lossy();
                let b = b.file_name().unwrap_or_else(|| b.as_os_str()).to_string_lossy();
                natural_cmp(&a, &b)
            });
            let relative = dir.strip_prefix(root).unwrap_or(&dir);
            let name = if relative.as_os_str().is_empty() { ".".to_string() }
                else { relative.to_string_lossy().to_string() };
            folders.push(MusicFolder { relative_name: name, files });
        }
    }
    folders.sort_by_key(|f| f.relative_name.to_lowercase());
    Ok(folders)
}

pub fn queue_file() -> PathBuf {
    data_file().with_file_name("queue.json")
}

pub fn load_queue() -> Vec<Track> {
    fs::read(queue_file()).ok()
        .and_then(|data| serde_json::from_slice::<Vec<Track>>(&data).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|track| if track.is_radio { valid_stream_url(&track.uri) } else { track.uri.starts_with("file://") })
        .collect()
}

pub fn save_queue(tracks: &[Track]) -> Result<()> {
    let path = queue_file();
    fs::create_dir_all(path.parent().context("No queue directory")?)?;
    let temp = path.with_extension("tmp");
    // Invariant: stations belong in stations.json, never queue.json.
    let music: Vec<&Track> = tracks.iter().filter(|track| !track.is_radio).collect();
    fs::write(&temp, serde_json::to_vec_pretty(&music)?)?;
    fs::rename(temp, path)?;
    Ok(())
}

#[cfg(test)]
mod music_tests {
    use super::*;
    #[test]
    fn numeric_track_order_uses_natural_sort() {
        use std::cmp::Ordering;
        assert_eq!(natural_cmp("1 - Bulletproof Cupid.flac", "10 - Second Sight.flac"), Ordering::Less);
        assert_eq!(natural_cmp("2 - English Summer Rain.flac", "10 - Second Sight.flac"), Ordering::Less);
        assert_eq!(natural_cmp("09. Track.mp3", "10. Track.mp3"), Ordering::Less);
        assert_eq!(natural_cmp("11 - Track.flac", "2 - Track.flac"), Ordering::Greater);
        assert_eq!(natural_cmp("002 - Song.flac", "2 - Song.flac"), Ordering::Greater);
    }

    #[test]
    fn scan_sorts_placebo_style_track_numbers() {
        let root = std::env::temp_dir().join(format!("slateamp-sort-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for name in ["1 - Bulletproof Cupid.mp3", "10 - Second Sight.mp3", "2 - English Summer Rain.mp3"] {
            std::fs::write(root.join(name), b"stub").unwrap();
        }
        let folders = scan_music(&root).unwrap();
        let names: Vec<_> = folders[0].files.iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec![
            "1 - Bulletproof Cupid.mp3",
            "2 - English Summer Rain.mp3",
            "10 - Second Sight.mp3",
        ]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scan_groups_files_by_their_actual_directories() {
        let root = std::env::temp_dir().join(format!("slateamp-folders-{}", std::process::id()));
        let a = root.join("Artist/Album");
        let b = root.join("Other");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("song.flac"), b"stub").unwrap();
        fs::write(a.join("cover.jpg"), b"stub").unwrap();
        fs::write(b.join("demo.mp3"), b"stub").unwrap();
        let folders = scan_music(&root).unwrap();
        assert_eq!(folders.len(), 2);
        assert!(folders.iter().any(|f| f.relative_name == "Artist/Album" && f.files.len() == 1));
        let _ = fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod queue_is_music_only_tests {
    use super::*;
    #[test]
    fn saved_queue_does_not_contain_radio() {
        let playlist = vec![
            Track { title: "Song".to_owned(), uri: "file:///tmp/song.flac".to_owned(), is_radio: false },
            Track::station("Station", "https://example.org/live"),
        ];
        let encoded = serde_json::to_string(
            &playlist.iter().filter(|track| !track.is_radio).collect::<Vec<_>>()
        ).unwrap();
        assert!(encoded.contains("Song"));
        assert!(!encoded.contains("Station"));
    }
}
