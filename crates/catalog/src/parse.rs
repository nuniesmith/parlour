//! File-name parsing for *arr/TRaSH-style libraries (C3, C5).
//!
//! Handles the shapes Radarr/Sonarr produce:
//! - `Movies/Some Movie (2019)/Some Movie (2019) [Bluray-1080p].mkv`
//! - `Shows/Some Show (2020)/Season 01/Some Show - S01E02 - The Episode.mkv`
//! - `Some Show 1x02.mkv`, multi-part movies (`... - cd1.mkv` files
//!   in one folder group into one item), samples and extras folders
//!   (skipped by rule, never catalogued).
//!
//! This is deliberately a tokeniser, not a regex: release-tag
//! stripping is a denylist of known tokens, so an unknown word is
//! always treated as part of the title rather than swallowed.

/// What a file name says about the item it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedName {
    pub title: String,
    pub year: Option<i32>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub episode_title: Option<String>,
    /// For episodes: the series title (from the file name when
    /// present, else the folder). For movies: same as `title`.
    pub series_title: Option<String>,
}

const RELEASE_TAGS: &[&str] = &[
    "1080p", "720p", "2160p", "480p", "576p", "4k", "uhd", "bluray", "blu-ray", "web-dl", "webdl",
    "webrip", "web", "hdrip", "dvdrip", "dvdscr", "brrip", "bdrip", "hdtv", "x264", "x265", "h264",
    "h265", "hevc", "avc", "aac", "ac3", "eac3", "dts", "truehd", "atmos", "remux", "proper",
    "repack", "extended", "unrated", "10bit", "hdr", "hdr10", "dovi", "dv", "hmax", "amzn", "nf",
    "dvd", "bd", "flac", "mp3", "dd5.1", "dd+", "ddp5.1", "5.1", "7.1", "2.0", "imax", "internal",
    "limited", "complete", "pack",
];

fn is_year_token(tok: &str) -> Option<i32> {
    let t = tok.trim_matches(|c| c == '(' || c == ')' || c == '[' || c == ']');
    if t.len() == 4 && t.chars().all(|c| c.is_ascii_digit()) {
        let y: i32 = t.parse().ok()?;
        if (1920..=2039).contains(&y) {
            return Some(y);
        }
    }
    None
}

/// Extract a `(YYYY)` year first (the TRaSH convention), else the
/// first standalone year-looking token.
fn extract_year(tokens: &[String]) -> Option<i32> {
    for t in tokens {
        if t.starts_with('(') {
            if let Some(y) = is_year_token(t) {
                return Some(y);
            }
        }
    }
    tokens.iter().find_map(|t| is_year_token(t))
}

/// Clean a raw title fragment: separators become spaces, bracketed
/// groups and release tags are dropped, a year is pulled out.
fn clean_title(raw: &str) -> (String, Option<i32>) {
    // Drop bracketed/braced groups entirely ([Bluray-1080p], {tmdb-123}).
    let mut no_brackets = String::new();
    let mut depth = 0i32;
    for c in raw.chars() {
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => depth = (depth - 1).max(0),
            _ if depth == 0 => no_brackets.push(c),
            _ => {}
        }
    }
    let spaced = no_brackets.replace(['.', '_'], " ");
    let tokens: Vec<String> = spaced.split_whitespace().map(str::to_string).collect();
    let year = extract_year(&tokens);
    // Remove the first year token, then the release tags.
    let mut out: Vec<String> = Vec::new();
    let mut year_dropped = false;
    for tok in tokens {
        let bare = tok.trim_matches(|c| c == '(' || c == ')').to_string();
        if !year_dropped && year.is_some() && is_year_token(&tok) == year {
            year_dropped = true;
            continue;
        }
        if RELEASE_TAGS.contains(&bare.to_lowercase().as_str()) {
            continue;
        }
        out.push(bare);
    }
    while matches!(out.last().map(String::as_str), Some("-") | Some("–")) {
        out.pop();
    }
    while matches!(out.first().map(String::as_str), Some("-") | Some("–")) {
        out.remove(0);
    }
    (out.join(" ").trim().to_string(), year)
}

/// Find an `SxxExx` (or `SxxExxExx`) pattern in a file stem.
/// Returns (season, episode, byte range of the match).
fn find_episode_pattern(stem: &str) -> Option<(i32, i32, usize, usize)> {
    let b = stem.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if (b[i] == b'S' || b[i] == b's') && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
            // season: 1-2 digits
            let mut j = i + 1;
            let mut season = String::new();
            while j < b.len() && b[j].is_ascii_digit() && season.len() < 2 {
                season.push(b[j] as char);
                j += 1;
            }
            if j < b.len() && (b[j] == b'E' || b[j] == b'e') {
                let mut k = j + 1;
                let mut episode = String::new();
                while k < b.len() && b[k].is_ascii_digit() && episode.len() < 3 {
                    episode.push(b[k] as char);
                    k += 1;
                }
                if !episode.is_empty() {
                    // Boundary check: char before S must not be a letter/digit.
                    let before_ok = i == 0 || !(b[i - 1].is_ascii_alphanumeric());
                    if before_ok {
                        return Some((season.parse().ok()?, episode.parse().ok()?, i, k));
                    }
                }
            }
        }
        i += 1;
    }
    None
}

/// Find an `NxNN` pattern (`1x02`). Returns (season, episode, range).
fn find_x_pattern(stem: &str) -> Option<(i32, i32, usize, usize)> {
    let b = stem.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i].is_ascii_digit() && (b[i + 1] == b'x' || b[i + 1] == b'X') {
            let mut k = i + 2;
            let mut episode = String::new();
            while k < b.len() && b[k].is_ascii_digit() && episode.len() < 3 {
                episode.push(b[k] as char);
                k += 1;
            }
            if episode.len() >= 2 {
                let season: i32 = (b[i] as char).to_string().parse().ok()?;
                return Some((season, episode.parse().ok()?, i, k));
            }
        }
        i += 1;
    }
    None
}

/// Parse a "Season 01" / "Season 1" / "S01" folder name.
pub fn season_from_folder(name: &str) -> Option<i32> {
    let lower = name.to_lowercase();
    let num: String = lower
        .trim_start_matches("season")
        .trim_start_matches('s')
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if num.is_empty() {
        None
    } else {
        num.parse().ok()
    }
}

/// Is this path a sample (by *arr rules: a `Sample` folder, or a
/// `sample` token in the file name)?
pub fn is_sample(rel_path: &str) -> bool {
    rel_path.split('/').any(|part| {
        let lower = part.to_lowercase();
        lower == "sample" || lower == "samples"
    }) || rel_path
        .rsplit('/')
        .next()
        .map(|f| {
            f.to_lowercase()
                .split(['.', ' ', '-', '_'])
                .any(|t| t == "sample")
        })
        .unwrap_or(false)
}

const EXTRA_DIRS: &[&str] = &[
    "extras",
    "extra",
    "featurettes",
    "deleted scenes",
    "behind the scenes",
    "trailers",
    "interviews",
    "scenes",
    "shorts",
    "other",
];

/// Is this path an extra (extras/featurettes/trailers folders, or a
/// `-trailer` file)? Extras are skipped by rule (C3).
pub fn is_extra(rel_path: &str) -> bool {
    let parts: Vec<&str> = rel_path.split('/').collect();
    for dir in &parts[..parts.len().saturating_sub(1)] {
        if EXTRA_DIRS.contains(&dir.to_lowercase().as_str()) {
            return true;
        }
    }
    let file = parts.last().copied().unwrap_or("").to_lowercase();
    file.contains("-trailer.") || file.contains(".trailer.") || file.ends_with("-trailer")
}

/// Parse one video file's library-relative path into a name.
/// `parent_titles` are the cleaned folder names above the file,
/// outermost first — the series/movie folder is the fallback title
/// source (TRaSH folders always carry the real title + year).
pub fn parse_file_name(rel_path: &str) -> ParsedName {
    let parts: Vec<&str> = rel_path.split('/').filter(|p| !p.is_empty()).collect();
    let file = parts.last().copied().unwrap_or("");
    let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
    let dirs: Vec<&str> = parts[..parts.len().saturating_sub(1)].to_vec();

    // Skip the immediate "Season N" folder when looking for a title folder.
    let title_dir = dirs
        .iter()
        .rev()
        .find(|d| season_from_folder(d).is_none())
        .copied();
    let season_dir = dirs.iter().rev().find_map(|d| season_from_folder(d));

    if let Some((season, episode, start, end)) =
        find_episode_pattern(stem).or_else(|| find_x_pattern(stem))
    {
        let before = &stem[..start];
        let after = &stem[end..];
        let (mut series_title, mut year) = clean_title(before);
        if series_title.is_empty() {
            if let Some(dir) = title_dir {
                let (t, y) = clean_title(dir);
                series_title = t;
                year = year.or(y);
            }
        } else if year.is_none() {
            if let Some(dir) = title_dir {
                let (_, y) = clean_title(dir);
                year = year.or(y);
            }
        }
        let (ep_title, _) = clean_title(after.trim_start_matches([' ', '-', '.', '_']));
        return ParsedName {
            title: if ep_title.is_empty() {
                format!("Episode {episode}")
            } else {
                ep_title.clone()
            },
            year,
            season: Some(season),
            episode: Some(episode),
            episode_title: if ep_title.is_empty() {
                None
            } else {
                Some(ep_title)
            },
            series_title: Some(series_title),
        };
    }

    // Movie (or an episode file with no pattern — treat as a
    // standalone item under its folder title).
    let (mut title, mut year) = clean_title(stem);
    if let Some(dir) = title_dir {
        let (dir_title, dir_year) = clean_title(dir);
        // TRaSH folders carry the canonical title; prefer the folder
        // when the file stem is a scene-style name or the folder has
        // a year the stem lacks.
        if !dir_title.is_empty() && (year.is_none() && dir_year.is_some()) {
            title = dir_title;
            year = dir_year;
        } else if title.is_empty() {
            title = dir_title;
            year = year.or(dir_year);
        }
    }
    let _ = season_dir; // a season folder without an episode pattern: still a movie-style item
    ParsedName {
        title,
        year,
        season: None,
        episode: None,
        episode_title: None,
        series_title: None,
    }
}

/// Lowercase alphanumeric key used for stable item identity.
pub fn normalize_key(s: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Sort title: leading articles move out of the way, lowercased.
pub fn sort_title(title: &str) -> String {
    let lower = title.to_lowercase();
    for article in ["the ", "a ", "an "] {
        if let Some(rest) = lower.strip_prefix(article) {
            return rest.to_string();
        }
    }
    lower
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movie_from_trash_naming() {
        let p = parse_file_name("Some Movie (2019)/Some Movie (2019) [Bluray-1080p].mkv");
        assert_eq!(p.title, "Some Movie");
        assert_eq!(p.year, Some(2019));
        assert_eq!(p.season, None);
    }

    #[test]
    fn movie_from_scene_stem() {
        let p = parse_file_name("Another.Film.2021.1080p.WEB-DL.x264.mkv");
        assert_eq!(p.title, "Another Film");
        assert_eq!(p.year, Some(2021));
    }

    #[test]
    fn episode_from_sxxexx() {
        let p = parse_file_name("Some Show (2020)/Season 01/Some Show - S01E02 - The Episode.mkv");
        assert_eq!(p.series_title.as_deref(), Some("Some Show"));
        assert_eq!(p.year, Some(2020));
        assert_eq!(p.season, Some(1));
        assert_eq!(p.episode, Some(2));
        assert_eq!(p.episode_title.as_deref(), Some("The Episode"));
    }

    #[test]
    fn episode_from_x_pattern_and_folder_title() {
        let p = parse_file_name("Old Show/Season 2/1x03.mkv");
        assert_eq!(p.series_title.as_deref(), Some("Old Show"));
        assert_eq!(p.season, Some(1));
        assert_eq!(p.episode, Some(3));
    }

    #[test]
    fn samples_and_extras_detected() {
        assert!(is_sample("Movie (2020)/Sample/movie-sample.mkv"));
        assert!(is_sample("Movie (2020)/movie.sample.mkv"));
        assert!(!is_sample("Movie (2020)/Movie (2020).mkv"));
        assert!(is_extra("Movie (2020)/Extras/deleted.mkv"));
        assert!(is_extra("Show/Season 01/Featurettes/making.mkv"));
        assert!(is_extra("Movie (2020)/Movie (2020)-trailer.mkv"));
        assert!(!is_extra("Movie (2020)/Movie (2020).mkv"));
    }

    #[test]
    fn keys_and_sort_titles() {
        assert_eq!(normalize_key("The Movie! (2019)"), "the-movie-2019");
        assert_eq!(sort_title("The Movie"), "movie");
        assert_eq!(sort_title("Movie"), "movie");
    }

    #[test]
    fn season_folder_parsing() {
        assert_eq!(season_from_folder("Season 01"), Some(1));
        assert_eq!(season_from_folder("Season 12"), Some(12));
        assert_eq!(season_from_folder("Specials"), None);
    }
}
