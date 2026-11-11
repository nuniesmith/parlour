//! The folder walker (C3).
//!
//! Recursively lists a library root: video files on the extension
//! allowlist, sidecar subtitle files, everything else ignored.
//! Samples and extras are filtered by the `parse` rules. Results
//! are sorted so scans are deterministic. Fingerprint = (size,
//! mtime) — the scanner compares it against the stored row and
//! skips probing unchanged files.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::parse::{is_extra, is_sample};

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "mov", "webm", "ts", "m2ts", "mpg", "mpeg", "wmv", "flv",
];

pub const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "vtt", "ass", "ssa", "sub", "sbv"];

#[derive(Debug, Clone)]
pub struct DiscoveredVideo {
    pub abs_path: String,
    pub rel_path: String,
    pub size_bytes: u64,
    pub mtime: SystemTime,
}

#[derive(Debug, Clone)]
pub struct SidecarSubtitle {
    pub abs_path: String,
    pub rel_path: String,
    /// Library-relative path of the matching video file stem
    /// (no extension, no `.en`/`.forced` suffixes) — the join key.
    pub video_stem_rel: String,
    pub format: String,
    pub language: Option<String>,
    pub is_forced: bool,
    pub is_sdh: bool,
}

#[derive(Debug, Default)]
pub struct WalkResult {
    pub videos: Vec<DiscoveredVideo>,
    pub sidecars: Vec<SidecarSubtitle>,
    pub skipped_samples: usize,
    pub skipped_extras: usize,
    pub errors: Vec<String>,
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

fn rel_string(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Split a sidecar file name into (video stem, language, forced, sdh):
/// `Movie (2019).en.forced.srt` -> (`Movie (2019)`, en, true, false).
pub fn split_sidecar_name(file_name: &str) -> (String, Option<String>, bool, bool) {
    let stem = file_name
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(file_name);
    let mut parts: Vec<&str> = stem.split('.').collect();
    let mut language = None;
    let mut forced = false;
    let mut sdh = false;
    while parts.len() > 1 {
        let last = parts[parts.len() - 1].to_lowercase();
        match last.as_str() {
            "forced" => forced = true,
            "sdh" | "cc" => sdh = true,
            "default" => {}
            t if (t.len() == 2 || t.len() == 3) && t.chars().all(|c| c.is_ascii_alphabetic()) => {
                language = Some(t.to_string());
            }
            _ => break,
        }
        parts.pop();
    }
    (parts.join("."), language, forced, sdh)
}

fn is_junk_dir(name: &str) -> bool {
    name.starts_with('.') || name == "@eaDir" || name == "#recycle" || name == ".AppleDouble"
}

pub fn walk_library(root: &Path) -> WalkResult {
    let mut result = WalkResult::default();
    if !root.is_dir() {
        result.errors.push(format!(
            "library path is not a directory: {}",
            root.display()
        ));
        return result;
    }
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                result
                    .errors
                    .push(format!("cannot read {}: {e}", dir.display()));
                continue;
            }
        };
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for path in paths {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            if path.is_dir() {
                if !is_junk_dir(&name) {
                    stack.push(path);
                }
                continue;
            }
            let rel = rel_string(root, &path);
            let ext = extension(&path);
            if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
                if is_sample(&rel) {
                    result.skipped_samples += 1;
                    continue;
                }
                if is_extra(&rel) {
                    result.skipped_extras += 1;
                    continue;
                }
                match std::fs::metadata(&path) {
                    Ok(md) => result.videos.push(DiscoveredVideo {
                        abs_path: path.to_string_lossy().to_string(),
                        rel_path: rel,
                        size_bytes: md.len(),
                        mtime: md.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    }),
                    Err(e) => result
                        .errors
                        .push(format!("cannot stat {}: {e}", path.display())),
                }
            } else if SUBTITLE_EXTENSIONS.contains(&ext.as_str()) {
                if is_sample(&rel) || is_extra(&rel) {
                    continue;
                }
                let (stem, language, forced, sdh) = split_sidecar_name(&name);
                let dir_rel = rel.rsplit_once('/').map(|(d, _)| d.to_string());
                let video_stem_rel = match dir_rel {
                    Some(d) => format!("{d}/{stem}"),
                    None => stem,
                };
                result.sidecars.push(SidecarSubtitle {
                    abs_path: path.to_string_lossy().to_string(),
                    rel_path: rel,
                    video_stem_rel,
                    format: ext,
                    language,
                    is_forced: forced,
                    is_sdh: sdh,
                });
            }
        }
    }
    result.videos.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    result.sidecars.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_names_split() {
        let (stem, lang, forced, sdh) = split_sidecar_name("Movie (2019).en.forced.srt");
        assert_eq!(stem, "Movie (2019)");
        assert_eq!(lang.as_deref(), Some("en"));
        assert!(forced);
        assert!(!sdh);

        let (stem, lang, forced, _) = split_sidecar_name("Show - S01E01.eng.srt");
        assert_eq!(stem, "Show - S01E01");
        assert_eq!(lang.as_deref(), Some("eng"));
        assert!(!forced);

        let (stem, lang, _, _) = split_sidecar_name("Plain.srt");
        assert_eq!(stem, "Plain");
        assert_eq!(lang, None);
    }

    #[test]
    fn walk_finds_videos_and_sidecars_and_skips_samples() {
        let dir = std::env::temp_dir().join(format!("parlour-walk-{}", std::process::id()));
        let movie_dir = dir.join("Some Movie (2019)");
        let sample_dir = movie_dir.join("Sample");
        std::fs::create_dir_all(&sample_dir).unwrap();
        std::fs::write(movie_dir.join("Some Movie (2019).mkv"), b"x").unwrap();
        std::fs::write(movie_dir.join("Some Movie (2019).en.srt"), b"sub").unwrap();
        std::fs::write(sample_dir.join("sample.mkv"), b"x").unwrap();
        std::fs::write(movie_dir.join("notes.txt"), b"x").unwrap();

        let result = walk_library(&dir);
        assert_eq!(result.videos.len(), 1, "{:?}", result.errors);
        assert_eq!(result.sidecars.len(), 1);
        assert_eq!(result.skipped_samples, 1);
        assert_eq!(
            result.videos[0].rel_path,
            "Some Movie (2019)/Some Movie (2019).mkv"
        );
        assert_eq!(
            result.sidecars[0].video_stem_rel,
            "Some Movie (2019)/Some Movie (2019)"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn walk_missing_root_is_an_error_not_a_panic() {
        let result = walk_library(Path::new("/nonexistent/parlour/root"));
        assert_eq!(result.videos.len(), 0);
        assert_eq!(result.errors.len(), 1);
    }
}
