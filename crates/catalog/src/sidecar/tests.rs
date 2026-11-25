//! Unit tests for the sidecar format and file handling (run in CI).
//! The database half is covered by `crates/server/tests/sidecar_flow.rs`.

use std::path::{Path, PathBuf};

use super::*;

const MOVIE: &[u8] = include_bytes!("../../tests/fixtures/sidecar-v1/movie.parlour.json");
const SERIES: &[u8] = include_bytes!("../../tests/fixtures/sidecar-v1/series.parlour.json");
const EPISODE: &[u8] = include_bytes!("../../tests/fixtures/sidecar-v1/episode.parlour.json");

fn json(text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}

fn problems_of(text: &str) -> Vec<String> {
    match parse(&json(text)) {
        Err(SidecarError::Invalid(p)) => p,
        other => panic!("expected rule problems, got {other:?}"),
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "parlour-sidecar-{name}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn each_fixture_parses_and_is_byte_for_byte_what_the_writer_writes() {
    for (name, bytes) in [("movie", MOVIE), ("series", SERIES), ("episode", EPISODE)] {
        let doc = parse(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(doc.kind.as_str(), name);
        assert_eq!(
            String::from_utf8(to_bytes(&doc)).unwrap(),
            String::from_utf8(bytes.to_vec()).unwrap(),
            "{name} is not the writer's own bytes"
        );
    }
}

#[test]
fn the_fixtures_carry_what_the_spec_says() {
    let movie = parse(MOVIE).unwrap();
    assert_eq!(movie.title.as_deref(), Some("The Lantern Keeper"));
    assert_eq!(movie.tvdb_id, None, "absent means not carried");
    let people = movie.people_in_order().unwrap();
    assert_eq!(people[1].sort_order, Some(1));
    assert_eq!(people[1].tmdb_id, None);
    let segments = movie.segments.as_ref().unwrap();
    assert_eq!(segments.size_bytes, 734_003_200);
    assert_eq!(movie.provider_id("tmdb"), Some(900_001));
    assert_eq!(movie.provider_id("tvdb"), None);

    let episode = parse(EPISODE).unwrap();
    assert_eq!(episode.season_number, Some(1));
    assert_eq!(
        episode.air_date,
        Some(chrono::NaiveDate::from_ymd_opt(2021, 3, 8).unwrap())
    );
    let marker = &episode.segments.as_ref().unwrap().entries[2];
    assert_eq!(
        (marker.kind.as_str(), marker.start_ms, marker.end_ms),
        ("credits", None, None),
        "a D21 marker has no times"
    );
    assert_eq!(episode.without_segments().segments, None);
}

#[test]
fn a_minimal_sidecar_carries_nothing_and_null_means_absent() {
    let doc = parse(&json(
        r#"{"format":"parlour-sidecar","format_version":1,"kind":"movie"}"#,
    ))
    .unwrap();
    assert_eq!(doc, Sidecar::new(SidecarKind::Movie));
    let with_nulls = parse(&json(
        r#"{"format":"parlour-sidecar","format_version":1,"kind":"movie",
            "title":null,"genres":null,"people":null,"segments":null}"#,
    ))
    .unwrap();
    assert_eq!(with_nulls, doc);
    assert_eq!(
        String::from_utf8(to_bytes(&doc)).unwrap(),
        "{\n  \"format\": \"parlour-sidecar\",\n  \"format_version\": 1,\n  \"kind\": \"movie\"\n}\n"
    );
}

#[test]
fn the_version_is_checked_before_anything_else() {
    // An unknown field and a bad kind, but the version speaks first.
    let err = parse(&json(
        r#"{"format":"parlour-sidecar","format_version":2,"kind":"book","shiny":true}"#,
    ))
    .unwrap_err();
    assert_eq!(err, SidecarError::Version("2".to_string()));
    assert!(err.to_string().contains("this server reads version 1"));
    assert_eq!(
        parse(&json(r#"{"format":"parlour-sidecar","kind":"movie"}"#)).unwrap_err(),
        SidecarError::Version("(missing)".to_string())
    );
    assert_eq!(
        parse(&json(
            r#"{"format":"parlour-sidecar","format_version":"1","kind":"movie"}"#
        ))
        .unwrap_err(),
        SidecarError::Version("\"1\"".to_string())
    );
}

#[test]
fn something_else_is_not_a_sidecar() {
    assert!(matches!(parse(b"{not json"), Err(SidecarError::Json(_))));
    assert_eq!(parse(b"[]").unwrap_err(), SidecarError::NotSidecar);
    assert_eq!(
        parse(&json(r#"{"format":"parlour-export","format_version":1}"#)).unwrap_err(),
        SidecarError::NotSidecar
    );
    assert_eq!(
        parse(&json(r#"{"title":"Some Movie"}"#)).unwrap_err(),
        SidecarError::NotSidecar
    );
}

#[test]
fn unknown_repeated_or_mistyped_fields_are_errors() {
    for text in [
        // unknown at the top
        r#"{"format":"parlour-sidecar","format_version":1,"kind":"movie","tagline":"x"}"#,
        // unknown in a credit
        r#"{"format":"parlour-sidecar","format_version":1,"kind":"movie",
            "people":[{"name":"A","role":"actor","age":40}]}"#,
        // unknown in a segment
        r#"{"format":"parlour-sidecar","format_version":1,"kind":"movie",
            "segments":{"size_bytes":1,"entries":[{"kind":"intro","source":"manual"}]}}"#,
        // repeated key
        r#"{"format":"parlour-sidecar","format_version":1,"kind":"movie","year":1,"year":2}"#,
        // wrong types
        r#"{"format":"parlour-sidecar","format_version":1,"kind":"movie","year":"2019"}"#,
        r#"{"format":"parlour-sidecar","format_version":1,"kind":"season"}"#,
        r#"{"format":"parlour-sidecar","format_version":1,"kind":"episode","air_date":"2021-02-30"}"#,
        // segments without their size
        r#"{"format":"parlour-sidecar","format_version":1,"kind":"movie","segments":{"entries":[]}}"#,
    ] {
        assert!(
            matches!(parse(&json(text)), Err(SidecarError::Shape(_))),
            "accepted: {text}"
        );
    }
}

#[test]
fn every_rule_is_checked() {
    let head = r#""format":"parlour-sidecar","format_version":1"#;
    let cases: &[(&str, &str)] = &[
        (r#""kind":"movie","title":"  ""#, "title is empty"),
        (r#""kind":"movie","overview":"""#, "overview is empty"),
        (
            r#""kind":"movie","content_rating":"""#,
            "content_rating is empty",
        ),
        (r#""kind":"movie","imdb_id":"""#, "imdb_id is empty"),
        (r#""kind":"movie","year":0"#, "year must be above 0"),
        (
            r#""kind":"movie","season_number":1"#,
            "season_number is only for an episode",
        ),
        (
            r#""kind":"series","episode_number":1"#,
            "episode_number is only for an episode",
        ),
        (
            r#""kind":"episode","season_number":-1"#,
            "season_number must be 0 or more",
        ),
        (
            r#""kind":"episode","episode_number":-1"#,
            "episode_number must be 0 or more",
        ),
        (
            r#""kind":"movie","genres":["Drama",""]"#,
            "genres has an empty entry",
        ),
        (
            r#""kind":"movie","genres":["Drama","Drama"]"#,
            "genres lists \"Drama\" twice",
        ),
        (
            r#""kind":"movie","vote_average":10.5"#,
            "vote_average must be between 0 and 10",
        ),
        (
            r#""kind":"movie","runtime_minutes":0"#,
            "runtime_minutes must be above 0",
        ),
        (r#""kind":"movie","tmdb_id":0"#, "tmdb_id must be above 0"),
        (r#""kind":"movie","tvdb_id":-4"#, "tvdb_id must be above 0"),
        (
            r#""kind":"movie","people":[{"name":"","role":"actor"}]"#,
            "people[0].name is empty",
        ),
        (
            r#""kind":"movie","people":[{"name":"A","role":" "}]"#,
            "people[0].role is empty",
        ),
        (
            r#""kind":"movie","people":[{"name":"A","role":"actor","character_name":""}]"#,
            "people[0].character_name is empty",
        ),
        (
            r#""kind":"movie","people":[{"name":"A","role":"actor","tmdb_id":0}]"#,
            "people[0].tmdb_id must be above 0",
        ),
        (
            r#""kind":"movie","people":[{"name":"A","role":"actor"},{"name":"A","role":"actor"}]"#,
            "people lists \"A\" as \"actor\" twice",
        ),
        (
            r#""kind":"series","segments":{"size_bytes":1,"entries":[]}"#,
            "segments are only for a movie or an episode",
        ),
        (
            r#""kind":"movie","segments":{"size_bytes":0,"entries":[]}"#,
            "segments.size_bytes must be above 0",
        ),
        (
            r#""kind":"movie","segments":{"size_bytes":1,"entries":[{"kind":"preview"}]}"#,
            "segments.entries[0].kind must be intro, credits or recap",
        ),
        (
            r#""kind":"movie","segments":{"size_bytes":1,"entries":[{"kind":"intro"},{"kind":"intro"}]}"#,
            "segments lists intro twice",
        ),
        (
            r#""kind":"movie","segments":{"size_bytes":1,"entries":[{"kind":"intro","start_ms":5,"end_ms":5}]}"#,
            "segments.entries[0] needs 0 <= start_ms < end_ms",
        ),
        (
            r#""kind":"movie","segments":{"size_bytes":1,"entries":[{"kind":"intro","start_ms":-1,"end_ms":5}]}"#,
            "segments.entries[0] needs 0 <= start_ms < end_ms",
        ),
        (
            r#""kind":"movie","segments":{"size_bytes":1,"entries":[{"kind":"intro","start_ms":5}]}"#,
            "segments.entries[0] has one time without the other",
        ),
    ];
    for (body, expected) in cases {
        let problems = problems_of(&format!("{{{head},{body}}}"));
        assert!(
            problems.iter().any(|p| p.starts_with(expected)),
            "{body}: expected {expected:?}, got {problems:?}"
        );
    }
    // The same person in two roles is fine; so is an episode 0.
    parse(&json(&format!(
        r#"{{{head},"kind":"episode","season_number":0,"episode_number":0,
            "people":[{{"name":"A","role":"actor"}},{{"name":"A","role":"director"}}]}}"#
    )))
    .unwrap();
}

#[test]
fn where_sidecars_live() {
    assert_eq!(
        file_sidecar_path(Path::new("/m/Movie (2019)/Movie (2019).mkv")),
        Some(PathBuf::from("/m/Movie (2019)/Movie (2019).parlour.json"))
    );
    assert_eq!(
        file_sidecar_path(Path::new("/m/a.b.c.mp4")),
        Some(PathBuf::from("/m/a.b.c.parlour.json"))
    );
    assert_eq!(file_sidecar_path(Path::new("/m/Show/series.mkv")), None);

    assert_eq!(
        series_folder(
            "/media/shows/Show (2020)/Season 01/Show - S01E02.mkv",
            "Show (2020)/Season 01/Show - S01E02.mkv"
        ),
        Some(PathBuf::from("/media/shows/Show (2020)"))
    );
    assert_eq!(
        series_folder(
            "/media/shows/Show/Show - S01E02.mkv",
            "Show/Show - S01E02.mkv"
        ),
        Some(PathBuf::from("/media/shows/Show"))
    );
    assert_eq!(
        series_folder(
            "/media/tv/Show/Specials/Show - S00E01.mkv",
            "Show/Specials/Show - S00E01.mkv"
        ),
        Some(PathBuf::from("/media/tv/Show")),
        "`Specials` counts as a season folder"
    );
    assert_eq!(
        series_folder("/media/shows/Show - S01E02.mkv", "Show - S01E02.mkv"),
        None,
        "an episode in the library root has no series folder"
    );
    assert_eq!(
        series_folder(
            "/media/shows/Season 1/x - S01E01.mkv",
            "Season 1/x - S01E01.mkv"
        ),
        None,
        "only season folders above it"
    );
    assert_eq!(
        series_folder("/Show/S1/Show - S01E01.mkv", "Show/S1/Show - S01E01.mkv"),
        Some(PathBuf::from("/Show"))
    );
}

#[test]
fn writes_are_atomic_idempotent_and_leave_no_temp_files() {
    let dir = scratch("write");
    let target = dir.join("Movie (2019).parlour.json");
    let doc = parse(MOVIE).unwrap();
    let bytes = to_bytes(&doc);

    assert_eq!(
        write_atomic(&target, &bytes).unwrap(),
        WriteOutcome::Written
    );
    assert_eq!(std::fs::read(&target).unwrap(), bytes);
    let mtime = std::fs::metadata(&target).unwrap().modified().unwrap();

    // Same bytes again: not rewritten (mtime unchanged), no temp file.
    assert_eq!(
        write_atomic(&target, &bytes).unwrap(),
        WriteOutcome::Unchanged
    );
    assert_eq!(
        std::fs::metadata(&target).unwrap().modified().unwrap(),
        mtime
    );

    // New bytes replace the file whole.
    let mut changed = doc.clone();
    changed.title = Some("The Lantern Keeper (Director's Cut)".to_string());
    let new_bytes = to_bytes(&changed);
    assert_eq!(
        write_atomic(&target, &new_bytes).unwrap(),
        WriteOutcome::Written
    );
    assert_eq!(parse(&std::fs::read(&target).unwrap()).unwrap(), changed);
    assert_eq!(entries(&dir), vec!["Movie (2019).parlour.json".to_string()]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_failed_write_leaves_the_old_sidecar_and_no_temp_file() {
    let dir = scratch("readonly");
    let target = dir.join("Movie.parlour.json");
    let old = to_bytes(&Sidecar::new(SidecarKind::Movie));
    write_atomic(&target, &old).unwrap();

    // A missing folder fails cleanly.
    assert!(write_atomic(&dir.join("gone/Movie.parlour.json"), &old).is_err());

    // A read-only folder (a read-only media mount) fails cleanly and
    // the old sidecar is untouched.
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let probe = std::fs::File::create(dir.join("probe"));
    if probe.is_ok() {
        // Running as root: permissions don't bind, nothing to test.
        let _ = std::fs::remove_file(dir.join("probe"));
    } else {
        let mut doc = Sidecar::new(SidecarKind::Movie);
        doc.title = Some("Changed".to_string());
        assert!(write_atomic(&target, &to_bytes(&doc)).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), old);
        assert_eq!(entries(&dir), vec!["Movie.parlour.json".to_string()]);
    }
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sidecar_kinds_follow_item_kinds() {
    assert_eq!(SidecarKind::for_item("movie"), Some(SidecarKind::Movie));
    assert_eq!(SidecarKind::for_item("series"), Some(SidecarKind::Series));
    assert_eq!(SidecarKind::for_item("episode"), Some(SidecarKind::Episode));
    assert_eq!(SidecarKind::for_item("season"), None);
    assert_eq!(SidecarKind::for_item("audiobook"), None);
}

// ------------------------------------------------ poster / backdrop (XFER5)

#[test]
fn only_jpegs_are_copied() {
    assert!(artwork::is_jpeg(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10]));
    assert!(!artwork::is_jpeg(b"\x89PNG\r\n\x1a\n"));
    assert!(!artwork::is_jpeg(b"<svg xmlns="));
    assert!(!artwork::is_jpeg(b"FAKEJPEGDATA"));
    assert!(!artwork::is_jpeg(&[]));
}

#[test]
fn a_copy_never_replaces_a_file_parlour_did_not_write() {
    use artwork::{decide, CopyAction};
    let ours = [0xFF, 0xD8, 0xFF, 1, 2, 3];
    let newer = [0xFF, 0xD8, 0xFF, 4, 5, 6];
    let ours_hash = crate::artwork::sha256_hex(&ours);
    // Nothing there: write.
    assert_eq!(decide(None, None, &ours), CopyAction::Write);
    // A leftover record without a file is no reason not to write.
    assert_eq!(decide(None, Some(&ours_hash), &ours), CopyAction::Write);
    // Same bytes: left alone, whoever wrote them.
    assert_eq!(
        decide(Some(&ours), Some(&ours_hash), &ours),
        CopyAction::Unchanged
    );
    assert_eq!(decide(Some(&ours), None, &ours), CopyAction::Unchanged);
    // Parlour's own earlier copy, and the cache has moved on: replace.
    assert_eq!(
        decide(Some(&ours), Some(&ours_hash), &newer),
        CopyAction::Replace
    );
    // A file Parlour never wrote: foreign, whatever it holds.
    assert_eq!(
        decide(Some(b"someone's poster"), None, &newer),
        CopyAction::Foreign
    );
    // Parlour wrote this path once, but someone has replaced the file
    // since: foreign too.
    assert_eq!(
        decide(Some(b"someone's poster"), Some(&ours_hash), &newer),
        CopyAction::Foreign
    );
}

#[test]
fn copies_are_named_poster_and_backdrop_jpg() {
    assert_eq!(
        artwork::ARTWORK_FILES,
        [("poster", "poster.jpg"), ("backdrop", "backdrop.jpg")]
    );
    // The scanner never takes them for videos or sidecars.
    for (_, name) in artwork::ARTWORK_FILES {
        let ext = Path::new(name).extension().unwrap().to_str().unwrap();
        assert!(!crate::walk::VIDEO_EXTENSIONS.contains(&ext), "{name}");
        assert!(!name.ends_with(FILE_SUFFIX), "{name}");
    }
}

// ------------------------------------------------------------ NFO (XFER6)

use super::nfo::{
    file_nfo_path, parse_nfo, to_nfo_bytes, Nfo, NfoContent, NfoError, NfoFields, MAX_NFO_BYTES,
};

const NFO_MOVIE: &[u8] = include_bytes!("../../tests/fixtures/nfo/movie.nfo");
const NFO_TVSHOW: &[u8] = include_bytes!("../../tests/fixtures/nfo/tvshow.nfo");
const NFO_EPISODE: &[u8] = include_bytes!("../../tests/fixtures/nfo/episode.nfo");
const NFO_OTHER: &[u8] = include_bytes!("../../tests/fixtures/nfo/other-software-movie.nfo");

fn nfo(text: &str) -> Result<Nfo, NfoError> {
    parse_nfo(text.as_bytes())
}

fn fields(text: &str) -> NfoFields {
    nfo(text).unwrap_or_else(|e| panic!("{e}")).fields
}

#[test]
fn parlour_nfo_fixtures_read_and_are_what_the_writer_writes() {
    let movie = parse_nfo(NFO_MOVIE).unwrap();
    assert_eq!(movie.kind, SidecarKind::Movie);
    assert_eq!(
        movie.fields,
        NfoFields {
            title: Some("The Lantern Keeper".into()),
            tmdb_id: Some(900001),
            tvdb_id: None,
            imdb_id: Some("tt9000001".into()),
        }
    );
    let written = to_nfo_bytes(
        SidecarKind::Movie,
        &NfoContent {
            title: Some("The Lantern Keeper".into()),
            year: Some(2019),
            tmdb_id: Some(900001),
            imdb_id: Some("tt9000001".into()),
            ..Default::default()
        },
    );
    assert_eq!(
        String::from_utf8(written).unwrap(),
        std::str::from_utf8(NFO_MOVIE).unwrap()
    );

    let show = parse_nfo(NFO_TVSHOW).unwrap();
    assert_eq!(show.kind, SidecarKind::Series);
    assert_eq!(
        (
            show.fields.tmdb_id,
            show.fields.tvdb_id,
            show.fields.imdb_id.as_deref()
        ),
        (Some(900002), Some(800002), Some("tt9000002"))
    );
    let written = to_nfo_bytes(
        SidecarKind::Series,
        &NfoContent {
            title: Some("Harbour Lights".into()),
            year: Some(2018),
            tmdb_id: Some(900002),
            tvdb_id: Some(800002),
            imdb_id: Some("tt9000002".into()),
            ..Default::default()
        },
    );
    assert_eq!(written, NFO_TVSHOW);

    let ep = parse_nfo(NFO_EPISODE).unwrap();
    assert_eq!(ep.kind, SidecarKind::Episode);
    assert_eq!((ep.season_number, ep.episode_number), (Some(1), Some(2)));
    assert_eq!(ep.fields.title.as_deref(), Some("Low Tide"));
    assert_eq!(ep.fields.tvdb_id, Some(800003));
    let written = to_nfo_bytes(
        SidecarKind::Episode,
        &NfoContent {
            title: Some("Low Tide".into()),
            // An episode's NFO carries no year.
            year: Some(2018),
            season_number: Some(1),
            episode_number: Some(2),
            tvdb_id: Some(800003),
            ..Default::default()
        },
    );
    assert_eq!(written, NFO_EPISODE);
}

#[test]
fn another_programs_nfo_gives_only_its_title_and_ids() {
    let n = parse_nfo(NFO_OTHER).unwrap();
    assert_eq!(n.kind, SidecarKind::Movie);
    assert_eq!(
        n.fields,
        NfoFields {
            // The entity is resolved; the nested <set><title> is not the
            // movie's title; plot, year, cast, ratings, <id>, an unknown
            // uniqueid type and an untyped one are all skipped; the empty
            // <tvdbid> is absent; the link after the root isn't read.
            title: Some("Salt & Iron".into()),
            tmdb_id: Some(900004),
            tvdb_id: None,
            imdb_id: Some("tt9000004".into()),
        }
    );
}

#[test]
fn an_nfo_is_read_leniently_around_what_parlour_uses() {
    // An empty root: a kind and nothing else.
    let n = nfo("<movie/>").unwrap();
    assert_eq!(
        (n.kind, n.fields),
        (SidecarKind::Movie, NfoFields::default())
    );
    // Root names are matched without regard to case; whitespace trimmed.
    assert_eq!(
        fields("<Movie><Title>  Spaced Out  </Title><TMDBID> 7 </TMDBID></Movie>"),
        NfoFields {
            title: Some("Spaced Out".into()),
            tmdb_id: Some(7),
            ..Default::default()
        }
    );
    // The same value twice (both id forms) is fine.
    assert_eq!(
        fields(r#"<tvshow><uniqueid type="TVDB">5</uniqueid><tvdbid>5</tvdbid></tvshow>"#).tvdb_id,
        Some(5)
    );
    // Season / episode mean nothing in a movie's NFO.
    assert_eq!(
        nfo("<movie><season>x</season></movie>")
            .unwrap()
            .season_number,
        None
    );
    // A multi-episode file's NFO: only the first episode is read.
    let n = nfo(
        "<episodedetails><title>One</title><episode>1</episode></episodedetails>\n\
         <episodedetails><title>Two</title><episode>2</episode></episodedetails>",
    )
    .unwrap();
    assert_eq!(
        (n.fields.title.as_deref(), n.episode_number),
        (Some("One"), Some(1))
    );
    // Character references and a doctype are fine.
    assert_eq!(
        fields("<!DOCTYPE movie><movie><title>Caf&#233; &#x26; Co</title></movie>").title,
        Some("Café & Co".into())
    );
}

#[test]
fn a_bad_nfo_is_refused_with_a_reason() {
    assert_eq!(parse_nfo(&[0xff, 0xfe, 0x00]), Err(NfoError::NotUtf8));
    assert_eq!(
        nfo("https://www.example.org/movie/1\n"),
        Err(NfoError::NoRoot)
    );
    assert_eq!(nfo(""), Err(NfoError::NoRoot));
    assert_eq!(
        nfo("<musicvideo/>"),
        Err(NfoError::Root("musicvideo".into()))
    );
    assert!(matches!(
        nfo("<movie><title>x</movie>"),
        Err(NfoError::Xml(_))
    ));
    assert!(matches!(
        nfo("<movie><title>x</title>"),
        Err(NfoError::Xml(_))
    ));
    assert!(matches!(
        nfo("<movie><title>&bogus;</title></movie>"),
        Err(NfoError::Xml(_))
    ));
    let value = |text: &str| match nfo(text) {
        Err(NfoError::Value(m)) => m,
        other => panic!("expected a value problem, got {other:?}"),
    };
    assert!(value("<movie><tmdbid>abc</tmdbid></movie>").contains("TMDB id"));
    assert!(value("<movie><tvdbid>0</tvdbid></movie>").contains("above 0"));
    assert!(value("<movie><imdbid>nm0000001</imdbid></movie>").contains("IMDb"));
    assert!(value("<episodedetails><season>-1</season></episodedetails>").contains("season"));
    assert!(
        value(r#"<movie><tmdbid>1</tmdbid><uniqueid type="tmdb">2</uniqueid></movie>"#)
            .contains("two different TMDB ids (1 and 2)")
    );
    assert!(value("<movie><title>A</title><title>B</title></movie>").contains("titles"));
    let mut big = b"<movie><plot>".to_vec();
    big.resize(MAX_NFO_BYTES as usize + 1, b'x');
    assert_eq!(parse_nfo(&big), Err(NfoError::TooLarge));
}

#[test]
fn the_nfo_writer_escapes_and_reads_back() {
    let content = NfoContent {
        title: Some("  <Tom> & \"Jerry's\"\u{1}  ".into()),
        year: Some(1999),
        tmdb_id: Some(42),
        tvdb_id: Some(0),
        imdb_id: Some("not-imdb".into()),
        ..Default::default()
    };
    let bytes = to_nfo_bytes(SidecarKind::Movie, &content);
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(
        text.contains("<title>&lt;Tom&gt; &amp; \"Jerry's\"</title>"),
        "{text}"
    );
    // Values no reader would accept are left out, not written.
    assert!(!text.contains("tvdb") && !text.contains("imdb"), "{text}");
    assert_eq!(
        parse_nfo(&bytes).unwrap().fields,
        NfoFields {
            title: Some("<Tom> & \"Jerry's\"".into()),
            tmdb_id: Some(42),
            ..Default::default()
        }
    );
    // Same content, same bytes.
    assert_eq!(bytes, to_nfo_bytes(SidecarKind::Movie, &content));
    // Nothing known: just the root.
    assert_eq!(
        to_nfo_bytes(SidecarKind::Series, &NfoContent::default()),
        b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<tvshow>\n</tvshow>\n"
    );
}

#[test]
fn nfo_names_follow_the_video_and_spare_the_folder_names() {
    assert_eq!(
        file_nfo_path(Path::new("/m/Arrival (2016)/Arrival (2016).mkv")),
        Some(PathBuf::from("/m/Arrival (2016)/Arrival (2016).nfo"))
    );
    assert_eq!(
        file_nfo_path(Path::new("/m/A/movie.mkv")),
        Some(PathBuf::from("/m/A/movie.nfo"))
    );
    assert_eq!(file_nfo_path(Path::new("/s/Show/tvshow.mkv")), None);
    assert_eq!(
        file_nfo_path(Path::new("/s/Show/Season 1/season.mp4")),
        None
    );
    // The scanner never takes an NFO for a video or a subtitle.
    assert!(!crate::walk::VIDEO_EXTENSIONS.contains(&nfo::NFO_EXTENSION));
    assert!(!crate::walk::SUBTITLE_EXTENSIONS.contains(&nfo::NFO_EXTENSION));
}
