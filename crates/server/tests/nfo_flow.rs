//! NFO interop (XFER6, D14), end to end against a real Postgres and a
//! **stub TMDB server** (no key, no internet), with real ffmpeg-made
//! files in a temp folder. Every NFO here is fictional.
//!
//! Covers:
//! - an NFO next to a movie is read for its title and ids, and its id
//!   picks the provider record (a search would find nothing); a
//!   library that hasn't opted in gets nothing written;
//! - `movie.nfo` counts in a folder that belongs to one movie and is
//!   ignored in a shared one; `tvshow.nfo` and an episode's NFO are
//!   read, and an episode NFO naming another episode is refused;
//! - a bad NFO is reported and the scan carries on;
//! - the sidecar beats the NFO field by field;
//! - a re-probed video gets its NFO's title back; an admin's fix-match
//!   beats the NFO; a removed NFO stops counting;
//! - with the opt-in on, the metadata job writes a `.nfo` beside each
//!   `.parlour.json` (and `tvshow.nfo`), a re-run writes nothing, the
//!   next scan doesn't take Parlour's own NFO for new input, and an NFO
//!   another program wrote is left alone and listed.
//!
//! Ignored by default: set PARLOUR_TEST_DATABASE_URL (a fresh
//! database of its own).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use axum::extract::{Path as AxumPath, Query};
use axum::routing::get;
use axum::{Json, Router};
use parlour_catalog::sidecar::nfo::{parse_nfo, NfoFields};
use parlour_catalog::sidecar::SidecarKind;
use parlour_metadata::tmdb::TmdbClient;
use serde_json::{json, Value};
use uuid::Uuid;

// ------------------------------------------------------------ stub TMDB

async fn stub_search_movie(Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    // Only "Test Movie" is findable by search; every NFO-named item
    // below can only be matched by its id.
    let results = match q.get("query").map(String::as_str) {
        Some("Test Movie") => json!([
            {"id": 101, "title": "Test Movie", "release_date": "2021-05-01"}
        ]),
        _ => json!([]),
    };
    Json(json!({ "results": results }))
}

async fn stub_search_tv(Query(_q): Query<HashMap<String, String>>) -> Json<Value> {
    Json(json!({ "results": [] }))
}

async fn stub_movie(AxumPath(id): AxumPath<i64>) -> Json<Value> {
    Json(match id {
        101 => json!({
            "id": 101, "title": "Test Movie", "release_date": "2021-05-01",
            "overview": "A test movie.", "runtime": 95, "vote_average": 7.5,
            "genres": [{"name": "Drama"}],
            "external_ids": {"imdb_id": "tt0000101"},
            "release_dates": {"results": []}, "credits": {"cast": []}
        }),
        201 => json!({
            "id": 201, "title": "Lantern Keeper", "release_date": "2019-02-02",
            "overview": "A keeper of a lighthouse.", "runtime": 88, "vote_average": 6.0,
            "genres": [{"name": "Mystery"}],
            "external_ids": {}, "release_dates": {"results": []}, "credits": {"cast": []}
        }),
        202 => json!({
            "id": 202, "title": "Salt and Iron", "release_date": "2011-01-01",
            "overview": "Another one.", "external_ids": {},
            "release_dates": {"results": []}, "credits": {"cast": []}
        }),
        _ => json!({"id": id, "title": "Stub", "external_ids": {},
                    "release_dates": {"results": []}, "credits": {"cast": []}}),
    })
}

async fn stub_tv(AxumPath(id): AxumPath<i64>) -> Json<Value> {
    Json(json!({
        "id": id, "name": "Harbour Lights", "first_air_date": "2018-03-01",
        "overview": "A show.", "episode_run_time": [42], "vote_average": 8.1,
        "genres": [{"name": "Drama"}],
        "external_ids": {"imdb_id": "tt0000301", "tvdb_id": 777},
        "content_ratings": {"results": []},
        "credits": {"cast": []}
    }))
}

async fn stub_season(AxumPath((_id, _season)): AxumPath<(i64, i32)>) -> Json<Value> {
    Json(json!({ "episodes": [
        {"season_number": 1, "episode_number": 1, "name": "First Light",
         "overview": "It begins.", "air_date": "2018-03-01", "runtime": 42},
        {"season_number": 1, "episode_number": 2, "name": "Low Tide",
         "overview": "It goes on.", "air_date": "2018-03-08", "runtime": 42}
    ]}))
}

fn stub_app() -> Router {
    Router::new()
        .route("/search/movie", get(stub_search_movie))
        .route("/search/tv", get(stub_search_tv))
        .route("/movie/{id}", get(stub_movie))
        .route("/tv/{id}", get(stub_tv))
        .route("/tv/{id}/season/{n}", get(stub_season))
}

// ------------------------------------------------------------ helpers

fn make_media(path: &Path, seconds: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let ok = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg(format!("testsrc=duration={seconds}:size=160x120:rate=10"))
        .args(["-f", "lavfi", "-i"])
        .arg(format!("sine=frequency=440:duration={seconds}"))
        .args([
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(path)
        .status()
        .expect("ffmpeg")
        .success();
    assert!(ok, "ffmpeg failed for {}", path.display());
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Every file under a folder, relative, with its bytes and mtime.
fn tree(root: &Path) -> BTreeMap<String, (Vec<u8>, SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let meta = std::fs::metadata(&path).unwrap();
                out.insert(
                    rel,
                    (std::fs::read(&path).unwrap(), meta.modified().unwrap()),
                );
            }
        }
    }
    out
}

fn written_names(root: &Path) -> Vec<String> {
    tree(root)
        .into_keys()
        .filter(|n| n.ends_with(".nfo") || n.ends_with(".parlour.json") || n.ends_with(".tmp"))
        .collect()
}

async fn create_library(pool: &parlour_db::PgPool, name: &str, root: &Path) -> Uuid {
    let (id,): (Uuid,) =
        sqlx::query_as("INSERT INTO libraries (name, kind) VALUES ($1, 'movies') RETURNING id")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO library_paths (library_id, path) VALUES ($1, $2)")
        .bind(id)
        .bind(root.to_string_lossy().to_string())
        .execute(pool)
        .await
        .unwrap();
    id
}

/// The item a file (matched by part of its relative path) belongs to:
/// the movie or episode itself, or its series for kind "series".
async fn item_id(pool: &parlour_db::PgPool, library: Uuid, kind: &str, path_like: &str) -> Uuid {
    let (id,): (Uuid,) = sqlx::query_as(
        "SELECT i.id FROM media_items i JOIN media_files f ON f.item_id = i.id OR f.item_id IN
             (SELECT e.id FROM media_items s JOIN media_items e ON e.parent_id = s.id
              WHERE s.parent_id = i.id)
         WHERE i.library_id = $1 AND i.kind = $2 AND f.relative_path LIKE $3
         LIMIT 1",
    )
    .bind(library)
    .bind(kind)
    .bind(format!("%{path_like}%"))
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("{kind} {path_like}: {e}"));
    id
}

#[derive(Debug, PartialEq)]
struct Ids {
    title: String,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    imdb_id: Option<String>,
    status: String,
}

async fn ids(pool: &parlour_db::PgPool, id: Uuid) -> Ids {
    let (title, tmdb_id, tvdb_id, imdb_id, status): (
        String,
        Option<i64>,
        Option<i64>,
        Option<String>,
        String,
    ) = sqlx::query_as(
        "SELECT title, tmdb_id, tvdb_id, imdb_id, metadata_status FROM media_items WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    Ids {
        title,
        tmdb_id,
        tvdb_id,
        imdb_id,
        status,
    }
}

async fn nfo_record(pool: &parlour_db::PgPool, item: Uuid) -> Option<(NfoFields, bool)> {
    let row: Option<(Value, bool)> = sqlx::query_as(
        "SELECT document, overridden_at IS NOT NULL FROM item_nfos WHERE item_id = $1",
    )
    .bind(item)
    .fetch_optional(pool)
    .await
    .unwrap();
    row.map(|(v, o)| (serde_json::from_value(v).unwrap(), o))
}

/// Run the real `fetch_metadata` job (as the worker would) and return
/// its result.
async fn metadata_job(
    pool: &parlour_db::PgPool,
    settings: &parlour_server::metadata::MetadataSettings,
    library: Uuid,
) -> Value {
    let id = parlour_jobs::enqueue(
        pool,
        parlour_jobs::JobKind::FetchMetadata,
        Some(library),
        json!({"trigger": "manual"}),
    )
    .await
    .unwrap();
    assert!(parlour_jobs::claim_by_id(pool, id).await.unwrap());
    let job = parlour_jobs::get(pool, id).await.unwrap().unwrap();
    parlour_server::metadata::run_metadata_job_claimed(pool.clone(), job, settings).await;
    let job = parlour_jobs::get(pool, id).await.unwrap().unwrap();
    assert_eq!(job.status, "succeeded", "{:?}", job.errors);
    assert_eq!(job.errors, json!([]), "job errors");
    job.result.unwrap()
}

// ------------------------------------------------------------ the flow

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn nfo_flow_end_to_end() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let pool = parlour_db::connect(&db_url).await.unwrap();
    parlour_db::migrate(&pool).await.unwrap();

    let stub_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stub_addr = stub_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(stub_listener, stub_app()).await.unwrap();
    });
    let stub_base = format!("http://{stub_addr}");
    let tmdb = TmdbClient::with_base("test-key".to_string(), &stub_base);
    let root: PathBuf =
        std::env::temp_dir().join(format!("parlour-nfo-flow-{}", Uuid::new_v4().simple()));
    let settings = parlour_server::metadata::MetadataSettings {
        tmdb_api_key: Some("test-key".to_string()),
        tmdb_base: Some(stub_base.clone()),
        artwork_dir: root.join("artwork"),
        ..Default::default()
    };

    // ---------------------------------------------------- 1. reading, no opt-in
    let lib = root.join("read");
    // A movie whose file name says nothing useful, with its NFO.
    make_media(&lib.join("lk/lk-final.mkv"), 2);
    write(
        &lib.join("lk/lk-final.nfo"),
        "<?xml version=\"1.0\"?>\n<movie>\n  <title>The Lantern Keeper</title>\n  \
         <plot>Skipped.</plot>\n  <year>1900</year>\n  <uniqueid type=\"tmdb\">201</uniqueid>\n  \
         <imdbid>tt9000001</imdbid>\n</movie>\n",
    );
    // A movie alone in its folder with a folder-level movie.nfo.
    make_media(&lib.join("si/si.mkv"), 2);
    write(
        &lib.join("si/movie.nfo"),
        "<movie><title>Salt &amp; Iron</title><tmdbid>202</tmdbid></movie>",
    );
    // Two movies sharing a folder: its movie.nfo can't say whose it is.
    make_media(&lib.join("pair/Alpha (2001).mkv"), 2);
    make_media(&lib.join("pair/Beta (2002).mkv"), 2);
    write(
        &lib.join("pair/movie.nfo"),
        "<movie><title>Whose?</title><tmdbid>101</tmdbid></movie>",
    );
    // A malformed NFO beside a movie the search can find.
    make_media(&lib.join("Test Movie (2021)/Test Movie (2021).mkv"), 2);
    write(
        &lib.join("Test Movie (2021)/Test Movie (2021).nfo"),
        "<movie><title>Broken</movie>",
    );
    // A series folder with tvshow.nfo, and episode NFOs (one naming
    // another episode).
    make_media(&lib.join("hl/Season 1/hl - S01E01.mkv"), 2);
    make_media(&lib.join("hl/Season 1/hl - S01E02.mkv"), 2);
    write(
        &lib.join("hl/tvshow.nfo"),
        "<tvshow><title>Harbour Lights</title><tmdbid>301</tmdbid><tvdbid>777</tvdbid></tvshow>",
    );
    write(
        &lib.join("hl/Season 1/hl - S01E01.nfo"),
        "<episodedetails><title>First Light</title><season>1</season><episode>1</episode>\
         <uniqueid type=\"tvdb\">8001</uniqueid></episodedetails>",
    );
    write(
        &lib.join("hl/Season 1/hl - S01E02.nfo"),
        "<episodedetails><title>Wrong One</title><season>1</season><episode>5</episode>\
         </episodedetails>",
    );
    let read_lib = create_library(&pool, "Read", &lib).await;
    let report = parlour_catalog::scan_library(&pool, read_lib, None)
        .await
        .unwrap();
    assert_eq!(
        report.seen_files, 7,
        "an .nfo is never catalogued as a video"
    );
    // lk, si, the series and its first episode.
    assert_eq!(report.nfos_applied, 4, "{report:?}");
    assert_eq!(report.errors.len(), 2, "{:?}", report.errors);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("Test Movie (2021).nfo") && e.contains("well-formed")),
        "a malformed NFO is reported with its path: {:?}",
        report.errors
    );
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("hl - S01E02.nfo") && e.contains("another season or episode")),
        "{:?}",
        report.errors
    );

    let lk = item_id(&pool, read_lib, "movie", "lk-final").await;
    assert_eq!(
        ids(&pool, lk).await,
        Ids {
            title: "The Lantern Keeper".into(),
            tmdb_id: Some(201),
            tvdb_id: None,
            imdb_id: Some("tt9000001".into()),
            status: "unmatched".into(),
        },
        "the NFO's title and ids, nothing else (its year and plot are skipped)"
    );
    let (y,): (Option<i32>,) = sqlx::query_as("SELECT year FROM media_items WHERE id = $1")
        .bind(lk)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(y, None, "the NFO's year is not read");
    let si = item_id(&pool, read_lib, "movie", "si/si").await;
    assert_eq!(ids(&pool, si).await.title, "Salt & Iron");
    for like in ["Alpha", "Beta"] {
        let m = item_id(&pool, read_lib, "movie", like).await;
        let got = ids(&pool, m).await;
        assert_eq!(
            (got.title.as_str(), got.tmdb_id),
            (like, None),
            "a shared folder's movie.nfo is ignored"
        );
        assert!(nfo_record(&pool, m).await.is_none());
    }
    let tm = item_id(&pool, read_lib, "movie", "Test Movie").await;
    assert_eq!(
        ids(&pool, tm).await.title,
        "Test Movie",
        "a bad NFO leaves the file name's title"
    );
    let show = item_id(&pool, read_lib, "series", "hl/").await;
    assert_eq!(
        (ids(&pool, show).await.title, ids(&pool, show).await.tmdb_id),
        ("Harbour Lights".to_string(), Some(301))
    );
    let ep1 = item_id(&pool, read_lib, "episode", "S01E01").await;
    assert_eq!(
        (ids(&pool, ep1).await.title, ids(&pool, ep1).await.tvdb_id),
        ("First Light".to_string(), Some(8001))
    );

    // The metadata job: the NFO's id picks the record (a search for these
    // titles finds nothing); nothing is written without the opt-in.
    let before = tree(&lib);
    let result = metadata_job(&pool, &settings, read_lib).await;
    assert_eq!(result["nfos_written"], 0, "{result}");
    assert_eq!(result["sidecars_written"], 0, "{result}");
    assert_eq!(tree(&lib), before, "no opt-in: nothing written or touched");
    let got = ids(&pool, lk).await;
    assert_eq!(
        (got.status.as_str(), got.tmdb_id, got.title.as_str()),
        ("matched", Some(201), "The Lantern Keeper"),
        "matched by the NFO's id; the provider doesn't change a movie's title"
    );
    assert_eq!(ids(&pool, si).await.status, "matched");
    assert_eq!(ids(&pool, show).await.status, "matched");
    assert_eq!(
        ids(&pool, tm).await.tmdb_id,
        Some(101),
        "the bad NFO's movie still matches by search"
    );
    let (overview,): (Option<String>,) =
        sqlx::query_as("SELECT overview FROM media_items WHERE id = $1")
            .bind(lk)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(overview.as_deref(), Some("A keeper of a lighthouse."));

    // A re-probed video gets its NFO's title back.
    make_media(&lib.join("lk/lk-final.mkv"), 3);
    let report = parlour_catalog::scan_library(&pool, read_lib, None)
        .await
        .unwrap();
    assert_eq!(report.probed_files, 1);
    assert_eq!(report.nfos_applied, 0, "the NFO itself didn't change");
    assert_eq!(ids(&pool, lk).await.title, "The Lantern Keeper");

    // ---------------------------------------------------- 2. sidecar beats NFO
    let sc_dir = lib.join("sc");
    make_media(&sc_dir.join("sc.mkv"), 2);
    write(
        &sc_dir.join("sc.parlour.json"),
        "{\n  \"format\": \"parlour-sidecar\",\n  \"format_version\": 1,\n  \
         \"kind\": \"movie\",\n  \"title\": \"Sidecar Title\",\n  \"tmdb_id\": 101\n}\n",
    );
    write(
        &sc_dir.join("sc.nfo"),
        "<movie><title>Nfo Title</title><tmdbid>202</tmdbid><imdbid>tt9000009</imdbid></movie>",
    );
    let report = parlour_catalog::scan_library(&pool, read_lib, None)
        .await
        .unwrap();
    assert_eq!((report.sidecars_applied, report.nfos_applied), (1, 1));
    let sc = item_id(&pool, read_lib, "movie", "sc/sc").await;
    let got = ids(&pool, sc).await;
    assert_eq!(
        (got.title.as_str(), got.tmdb_id, got.imdb_id.as_deref()),
        ("Sidecar Title", Some(101), Some("tt9000009")),
        "field by field: the sidecar's title and id, the NFO's IMDb id"
    );
    metadata_job(&pool, &settings, read_lib).await;
    assert_eq!(
        ids(&pool, sc).await.tmdb_id,
        Some(101),
        "the sidecar's id picks the record, not the NFO's"
    );

    // ---------------------------------------------------- 3. fix-match, removal
    parlour_catalog::apply_manual(&pool, &tmdb, lk, 202)
        .await
        .unwrap();
    assert!(
        nfo_record(&pool, lk).await.unwrap().1,
        "the fix overrides the NFO"
    );
    metadata_job(&pool, &settings, read_lib).await;
    assert_eq!(
        ids(&pool, lk).await.tmdb_id,
        Some(202),
        "a library match doesn't go back to the NFO's id after a fix"
    );
    std::fs::remove_file(lib.join("si/movie.nfo")).unwrap();
    parlour_catalog::scan_library(&pool, read_lib, None)
        .await
        .unwrap();
    assert!(
        nfo_record(&pool, si).await.is_none(),
        "a removed NFO stops counting"
    );
    assert_eq!(ids(&pool, si).await.title, "Salt & Iron", "its values stay");

    // ---------------------------------------------------- 4. writing (opt-in)
    let wlib = root.join("write");
    make_media(&wlib.join("Test Movie (2021)/Test Movie (2021).mkv"), 2);
    make_media(&wlib.join("hl/Season 1/hl - S01E01.mkv"), 2);
    // The series is matched through a hand-written sidecar, so Parlour
    // writes its tvshow.nfo fresh.
    write(
        &wlib.join("hl/series.parlour.json"),
        "{\n  \"format\": \"parlour-sidecar\",\n  \"format_version\": 1,\n  \
         \"kind\": \"series\",\n  \"title\": \"Harbour Lights\",\n  \"tmdb_id\": 301\n}\n",
    );
    // Another program's NFO beside an episode: Parlour must not replace it.
    let foreign = "<?xml version=\"1.0\"?>\n<episodedetails>\n<title>First Light</title>\n\
                   <plot>Kept as it is.</plot>\n</episodedetails>\n";
    write(&wlib.join("hl/Season 1/hl - S01E01.nfo"), foreign);
    let write_lib = create_library(&pool, "Write", &wlib).await;
    sqlx::query("UPDATE libraries SET write_sidecars = true WHERE id = $1")
        .bind(write_lib)
        .execute(&pool)
        .await
        .unwrap();
    let report = parlour_catalog::scan_library(&pool, write_lib, None)
        .await
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        (report.sidecars_applied, report.nfos_applied),
        (1, 1),
        "the series' sidecar; the episode's foreign NFO"
    );
    let result = metadata_job(&pool, &settings, write_lib).await;
    assert_eq!(result["matched"], 2, "{result}");
    assert_eq!(
        result["nfos_written"], 2,
        "the movie's NFO and tvshow.nfo: {result}"
    );
    let left: Vec<String> = serde_json::from_value(result["nfos_left_alone"].clone()).unwrap();
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(left[0].contains("hl - S01E01.nfo"), "{left:?}");
    let tvshow = parse_nfo(&std::fs::read(wlib.join("hl/tvshow.nfo")).unwrap()).unwrap();
    assert_eq!(tvshow.kind, SidecarKind::Series);
    assert_eq!(
        (
            tvshow.fields.title.as_deref(),
            tvshow.fields.tmdb_id,
            tvshow.fields.tvdb_id
        ),
        (Some("Harbour Lights"), Some(301), Some(777))
    );
    assert_eq!(
        std::fs::read_to_string(wlib.join("hl/Season 1/hl - S01E01.nfo")).unwrap(),
        foreign,
        "an NFO another program wrote is never replaced"
    );
    let names = written_names(&wlib);
    assert_eq!(
        names,
        vec![
            "Test Movie (2021)/Test Movie (2021).nfo",
            "Test Movie (2021)/Test Movie (2021).parlour.json",
            "hl/Season 1/hl - S01E01.nfo",
            "hl/Season 1/hl - S01E01.parlour.json",
            "hl/series.parlour.json",
            "hl/tvshow.nfo",
        ],
        "an NFO beside each sidecar, no temp files"
    );
    let written =
        parse_nfo(&std::fs::read(wlib.join("Test Movie (2021)/Test Movie (2021).nfo")).unwrap())
            .unwrap();
    assert_eq!(written.kind, SidecarKind::Movie);
    assert_eq!(
        written.fields,
        NfoFields {
            title: Some("Test Movie".into()),
            tmdb_id: Some(101),
            tvdb_id: None,
            imdb_id: Some("tt0000101".into()),
        }
    );
    let text =
        std::fs::read_to_string(wlib.join("Test Movie (2021)/Test Movie (2021).nfo")).unwrap();
    assert!(text.contains("<year>2021</year>"), "{text}");
    assert!(
        !text.contains("overview") && !text.contains("plot"),
        "identifying fields only"
    );

    // A re-run writes nothing, and the next scan doesn't take Parlour's
    // own NFO for new input.
    let before = tree(&wlib);
    let result = metadata_job(&pool, &settings, write_lib).await;
    assert_eq!(
        (
            result["nfos_written"].clone(),
            result["sidecars_written"].clone()
        ),
        (json!(0), json!(0)),
        "{result}"
    );
    assert_eq!(
        tree(&wlib),
        before,
        "same bytes: left alone, mtimes included"
    );
    let report = parlour_catalog::scan_library(&pool, write_lib, None)
        .await
        .unwrap();
    assert_eq!(report.nfos_applied, 0, "Parlour's own NFO isn't new input");
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    // Parlour's own NFO follows the item; once someone else edits it, it
    // is theirs.
    let wm = item_id(&pool, write_lib, "movie", "Test Movie").await;
    let own = wlib.join("Test Movie (2021)/Test Movie (2021).nfo");
    sqlx::query("UPDATE media_items SET imdb_id = 'tt0000999' WHERE id = $1")
        .bind(wm)
        .execute(&pool)
        .await
        .unwrap();
    let r = parlour_catalog::write_item_sidecars(&pool, wm)
        .await
        .unwrap();
    assert_eq!(r.nfos_written, 1, "{r:?}");
    assert!(std::fs::read_to_string(&own).unwrap().contains("tt0000999"));
    let edited = std::fs::read_to_string(&own)
        .unwrap()
        .replace("</movie>", "<plot>Mine now.</plot>\n</movie>");
    std::fs::write(&own, &edited).unwrap();
    let r = parlour_catalog::write_item_sidecars(&pool, wm)
        .await
        .unwrap();
    assert_eq!((r.nfos_written, r.nfos_left_alone.len()), (0, 1), "{r:?}");
    assert_eq!(std::fs::read_to_string(&own).unwrap(), edited);

    // ---------------------------------------------------- 5. opt-in off again
    sqlx::query("UPDATE libraries SET write_sidecars = false WHERE id = $1")
        .bind(write_lib)
        .execute(&pool)
        .await
        .unwrap();
    std::fs::remove_file(&own).unwrap();
    let before = tree(&wlib);
    let result = metadata_job(&pool, &settings, write_lib).await;
    assert_eq!(result["nfos_written"], 0, "{result}");
    assert_eq!(
        tree(&wlib),
        before,
        "with the opt-in off nothing is written"
    );

    let _ = std::fs::remove_dir_all(&root);
}
