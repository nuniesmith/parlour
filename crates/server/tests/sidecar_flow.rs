//! Sidecar metadata with the media files (XFER4, D14), end to end
//! against a real Postgres and a **stub TMDB server** (no key, no
//! internet), with real ffmpeg-made files in a temp folder.
//!
//! Covers:
//! - a folder with no sidecar scans as before, and a library that
//!   hasn't opted in never gets a file written (not even a temp file);
//! - the metadata job writes the sidecars of matched items in an
//!   opted-in library (movie, series folder, episode; a manual
//!   segment with its video's size), atomically, and re-running it
//!   rewrites nothing;
//! - those sidecars round-trip: a copy of the folder scanned as a new
//!   library gets the same metadata without any provider lookup;
//! - a sidecar's fields win over the provider (its id picks the
//!   record, its overview and genres survive the lookup), and fields
//!   it doesn't carry still come from the provider;
//! - a re-probed video gets its sidecar's title back; a bad sidecar is
//!   reported and skipped; segments for another encode are left out;
//!   an admin's fix-match wins over the sidecar (rewriting it where
//!   the library writes); a removed sidecar stops counting.
//!
//! Ignored by default: set PARLOUR_TEST_DATABASE_URL (a fresh
//! database of its own).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use axum::extract::{Path as AxumPath, Query};
use axum::routing::get;
use axum::{Json, Router};
use parlour_catalog::sidecar::{self, Sidecar};
use parlour_metadata::tmdb::TmdbClient;
use serde_json::{json, Value};
use uuid::Uuid;

// ------------------------------------------------------------ stub TMDB

async fn stub_search_movie(Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let results = match q.get("query").map(String::as_str) {
        Some("Test Movie") => json!([
            {"id": 101, "title": "Test Movie", "release_date": "2021-05-01",
             "overview": "A test.", "vote_average": 7.5}
        ]),
        Some("Obscure Thing") | Some("Obscure Thing Restored") => json!([
            {"id": 999, "title": "Totally Different", "release_date": "2001-01-01"}
        ]),
        _ => json!([]),
    };
    Json(json!({ "results": results }))
}

async fn stub_search_tv(Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let results = match q.get("query").map(String::as_str) {
        Some("Test Show") => json!([
            {"id": 301, "name": "Test Show", "first_air_date": "2020-03-01",
             "overview": "A show.", "vote_average": 8.1}
        ]),
        _ => json!([]),
    };
    Json(json!({ "results": results }))
}

async fn stub_movie(AxumPath(id): AxumPath<i64>) -> Json<Value> {
    Json(match id {
        101 => json!({
            "id": 101, "title": "Test Movie", "release_date": "2021-05-01",
            "overview": "A test movie.", "runtime": 95, "vote_average": 7.5,
            "genres": [{"name": "Drama"}],
            "external_ids": {"imdb_id": "tt0000101", "tvdb_id": null},
            "release_dates": {"results": [
                {"iso_3166_1": "US", "release_dates": [{"certification": "PG"}]}
            ]},
            "credits": {"cast": [
                {"name": "Ada Actor", "character": "Lead", "order": 0, "id": 501}
            ]}
        }),
        201 => json!({
            "id": 201, "title": "Ambiguous Tale", "release_date": "2020-01-01",
            "overview": "One of them.", "runtime": 88, "vote_average": 6.0,
            "genres": [{"name": "Comedy"}],
            "external_ids": {}, "release_dates": {"results": []},
            "credits": {"cast": [
                {"name": "Bo Player", "character": "Twin", "order": 0, "id": 502}
            ]}
        }),
        _ => json!({"id": id, "title": "Stub"}),
    })
}

async fn stub_tv(AxumPath(id): AxumPath<i64>) -> Json<Value> {
    Json(json!({
        "id": id, "name": "Test Show", "first_air_date": "2020-03-01",
        "overview": "A show.", "episode_run_time": [42], "vote_average": 8.1,
        "genres": [{"name": "Sci-Fi"}],
        "external_ids": {"imdb_id": "tt0000301", "tvdb_id": 777},
        "content_ratings": {"results": [
            {"iso_3166_1": "US", "rating": "TV-PG"}
        ]},
        "credits": {"cast": []}
    }))
}

async fn stub_season(AxumPath((id, season)): AxumPath<(i64, i32)>) -> Json<Value> {
    let _ = (id, season);
    Json(json!({ "episodes": [
        {"season_number": 1, "episode_number": 1, "name": "The Beginning",
         "overview": "It begins.", "air_date": "2020-03-01", "runtime": 42}
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

fn sidecar_names(root: &Path) -> Vec<String> {
    tree(root)
        .into_keys()
        .filter(|n| n.ends_with(".parlour.json") || n.ends_with(".tmp"))
        .collect()
}

fn copy_tree(from: &Path, to: &Path) {
    for rel in tree(from).keys() {
        let dest = to.join(rel);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(from.join(rel), dest).unwrap();
    }
}

fn read_sidecar(path: &Path) -> Sidecar {
    sidecar::parse(&std::fs::read(path).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
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

async fn item_id(pool: &parlour_db::PgPool, library: Uuid, kind: &str, title_like: &str) -> Uuid {
    let (id,): (Uuid,) = sqlx::query_as(
        "SELECT i.id FROM media_items i JOIN media_files f ON f.item_id = i.id OR f.item_id IN
             (SELECT e.id FROM media_items s JOIN media_items e ON e.parent_id = s.id
              WHERE s.parent_id = i.id)
         WHERE i.library_id = $1 AND i.kind = $2 AND f.relative_path LIKE $3
         LIMIT 1",
    )
    .bind(library)
    .bind(kind)
    .bind(format!("%{title_like}%"))
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("{kind} {title_like}: {e}"));
    id
}

#[derive(Debug, PartialEq)]
struct Meta {
    title: String,
    year: Option<i32>,
    overview: Option<String>,
    genres: Vec<String>,
    content_rating: Option<String>,
    vote_average: Option<f64>,
    runtime_minutes: Option<i32>,
    air_date: Option<chrono::NaiveDate>,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    imdb_id: Option<String>,
    people: Vec<(String, String, Option<String>, i32)>,
    manual_segments: Vec<(String, Option<i64>, Option<i64>)>,
}

type MetaRow = (
    String,
    Option<i32>,
    Option<String>,
    Vec<String>,
    Option<String>,
    Option<f64>,
    Option<i32>,
    Option<chrono::NaiveDate>,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

async fn meta(pool: &parlour_db::PgPool, id: Uuid) -> Meta {
    let r: MetaRow = sqlx::query_as(
        "SELECT title, year, overview, genres, content_rating, vote_average, runtime_minutes,
                air_date, tmdb_id, tvdb_id, imdb_id
         FROM media_items WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    let people = sqlx::query_as(
        "SELECT p.name, ip.role, ip.character_name, ip.sort_order
         FROM item_people ip JOIN people p ON p.id = ip.person_id
         WHERE ip.item_id = $1 ORDER BY ip.sort_order, p.name",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .unwrap();
    let manual_segments = sqlx::query_as(
        "SELECT kind, start_ms, end_ms FROM media_segments
         WHERE item_id = $1 AND source = 'manual' ORDER BY kind",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .unwrap();
    Meta {
        title: r.0,
        year: r.1,
        overview: r.2,
        genres: r.3,
        content_rating: r.4,
        vote_average: r.5,
        runtime_minutes: r.6,
        air_date: r.7,
        tmdb_id: r.8,
        tvdb_id: r.9,
        imdb_id: r.10,
        people,
        manual_segments,
    }
}

async fn stored_rows(pool: &parlour_db::PgPool, library: Uuid) -> i64 {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM item_sidecars s JOIN media_items i ON i.id = s.item_id
         WHERE i.library_id = $1",
    )
    .bind(library)
    .fetch_one(pool)
    .await
    .unwrap();
    n
}

async fn overridden(pool: &parlour_db::PgPool, item: Uuid) -> bool {
    let (b,): (bool,) =
        sqlx::query_as("SELECT overridden_at IS NOT NULL FROM item_sidecars WHERE item_id = $1")
            .bind(item)
            .fetch_one(pool)
            .await
            .unwrap();
    b
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
async fn sidecar_flow_end_to_end() {
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
        std::env::temp_dir().join(format!("parlour-sidecar-flow-{}", Uuid::new_v4().simple()));
    let settings = parlour_server::metadata::MetadataSettings {
        tmdb_api_key: Some("test-key".to_string()),
        tmdb_base: Some(stub_base.clone()),
        artwork_dir: root.join("artwork"),
        ..Default::default()
    };

    // ---------------------------------------------------- 1. no sidecar, no opt-in
    let plain = root.join("plain");
    make_media(&plain.join("Test Movie (2021)/Test Movie (2021).mkv"), 2);
    let plain_lib = create_library(&pool, "Plain", &plain).await;
    let before = tree(&plain);
    let report = parlour_catalog::scan_library(&pool, plain_lib, None)
        .await
        .unwrap();
    assert_eq!(
        (
            report.seen_files,
            report.created_items,
            report.sidecars_applied
        ),
        (1, 1, 0)
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let plain_movie = item_id(&pool, plain_lib, "movie", "Test Movie").await;
    let m = meta(&pool, plain_movie).await;
    assert_eq!(
        (m.title.as_str(), m.year, m.overview.as_deref()),
        ("Test Movie", Some(2021), None),
        "without a sidecar the scan gives the file name's title and year, as before"
    );
    assert_eq!(stored_rows(&pool, plain_lib).await, 0);
    let result = metadata_job(&pool, &settings, plain_lib).await;
    assert_eq!(result["matched"], 1);
    assert_eq!(result["sidecars_written"], 0);
    let m = meta(&pool, plain_movie).await;
    assert_eq!(m.overview.as_deref(), Some("A test movie."));
    assert_eq!(m.tmdb_id, Some(101));
    assert_eq!(
        tree(&plain),
        before,
        "a library that hasn't opted in: nothing written, renamed or touched"
    );

    // ---------------------------------------------------- 2. the job writes (opt-in)
    let source = root.join("source");
    make_media(&source.join("Test Movie (2021)/Test Movie (2021).mkv"), 3);
    make_media(&source.join("Test Show/Season 1/Test Show - S01E01.mkv"), 2);
    make_media(
        &source.join("Obscure Thing (1984)/Obscure Thing (1984).mkv"),
        2,
    );
    let source_lib = create_library(&pool, "Source", &source).await;
    // XFER5 will expose this flag; here the test sets it directly.
    sqlx::query("UPDATE libraries SET write_sidecars = true WHERE id = $1")
        .bind(source_lib)
        .execute(&pool)
        .await
        .unwrap();
    let report = parlour_catalog::scan_library(&pool, source_lib, None)
        .await
        .unwrap();
    assert_eq!(report.sidecars_applied, 0);
    assert!(
        sidecar_names(&source).is_empty(),
        "a scan never writes sidecars"
    );
    let movie = item_id(&pool, source_lib, "movie", "Test Movie").await;
    let show = item_id(&pool, source_lib, "series", "Test Show").await;
    let episode = item_id(&pool, source_lib, "episode", "S01E01").await;
    // A manual intro (SEG4) and a credits "none" marker (D21) on the
    // movie: segments travel in the sidecar of its playback file.
    sqlx::query(
        "INSERT INTO media_segments (item_id, kind, start_ms, end_ms, source, confidence)
         VALUES ($1, 'intro', 100, 900, 'manual', 1.0),
                ($1, 'credits', NULL, NULL, 'manual', 1.0),
                ($1, 'recap', 0, 50, 'chapters', 1.0)",
    )
    .bind(movie)
    .execute(&pool)
    .await
    .unwrap();

    let result = metadata_job(&pool, &settings, source_lib).await;
    assert_eq!(result["matched"], 2, "{result}");
    assert_eq!(result["unmatched"], 1, "{result}");
    assert_eq!(result["sidecars_written"], 3, "{result}");
    assert_eq!(
        sidecar_names(&source),
        vec![
            "Test Movie (2021)/Test Movie (2021).parlour.json".to_string(),
            "Test Show/Season 1/Test Show - S01E01.parlour.json".to_string(),
            "Test Show/series.parlour.json".to_string(),
        ],
        "matched items only, no temp files left, nothing for the unmatched one"
    );
    let movie_size = std::fs::metadata(source.join("Test Movie (2021)/Test Movie (2021).mkv"))
        .unwrap()
        .len() as i64;
    let doc = read_sidecar(&source.join("Test Movie (2021)/Test Movie (2021).parlour.json"));
    assert_eq!(doc.kind, sidecar::SidecarKind::Movie);
    assert_eq!(
        (
            doc.title.as_deref(),
            doc.year,
            doc.overview.as_deref(),
            doc.tmdb_id,
            doc.imdb_id.as_deref(),
            doc.content_rating.as_deref(),
            doc.runtime_minutes,
        ),
        (
            Some("Test Movie"),
            Some(2021),
            Some("A test movie."),
            Some(101),
            Some("tt0000101"),
            Some("PG"),
            Some(95),
        )
    );
    assert_eq!(doc.genres, Some(vec!["Drama".to_string()]));
    let people = doc.people.clone().unwrap();
    assert_eq!(
        (
            people[0].name.as_str(),
            people[0].role.as_str(),
            people[0].tmdb_id
        ),
        ("Ada Actor", "actor", Some(501))
    );
    let segments = doc.segments.clone().unwrap();
    assert_eq!(segments.size_bytes, movie_size);
    assert_eq!(
        segments
            .entries
            .iter()
            .map(|s| (s.kind.as_str(), s.start_ms, s.end_ms))
            .collect::<Vec<_>>(),
        vec![("intro", Some(100), Some(900)), ("credits", None, None)],
        "manual rows only (the chapters recap stays behind)"
    );
    let series_doc = read_sidecar(&source.join("Test Show/series.parlour.json"));
    assert_eq!(
        (
            series_doc.tmdb_id,
            series_doc.tvdb_id,
            series_doc.segments.is_none()
        ),
        (Some(301), Some(777), true)
    );
    let ep_doc = read_sidecar(&source.join("Test Show/Season 1/Test Show - S01E01.parlour.json"));
    assert_eq!(
        (
            ep_doc.title.as_deref(),
            ep_doc.season_number,
            ep_doc.episode_number,
            ep_doc.overview.as_deref()
        ),
        (Some("The Beginning"), Some(1), Some(1), Some("It begins."))
    );
    assert_eq!(stored_rows(&pool, source_lib).await, 3);

    // Re-running the job (and a scan) rewrites nothing.
    let written = tree(&source);
    let result = metadata_job(&pool, &settings, source_lib).await;
    assert_eq!(result["sidecars_written"], 0, "{result}");
    let report = parlour_catalog::scan_library(&pool, source_lib, None)
        .await
        .unwrap();
    assert_eq!(
        report.sidecars_applied, 0,
        "its own sidecars are already read"
    );
    assert_eq!(
        tree(&source),
        written,
        "idempotent: same bytes, same mtimes"
    );

    // ---------------------------------------------------- 3. round trip to a new library
    let moved = root.join("moved");
    copy_tree(&source, &moved);
    let moved_lib = create_library(&pool, "Moved", &moved).await;
    let moved_before = tree(&moved);
    let report = parlour_catalog::scan_library(&pool, moved_lib, None)
        .await
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.sidecars_applied, 3);
    for (kind, like, original) in [
        ("movie", "Test Movie", movie),
        ("series", "Test Show", show),
        ("episode", "S01E01", episode),
    ] {
        let copy = item_id(&pool, moved_lib, kind, like).await;
        assert_eq!(
            meta(&pool, copy).await,
            meta(&pool, original).await,
            "{kind}: the scan alone, with no provider lookup, carries the metadata"
        );
    }

    // ---------------------------------------------------- 4. the sidecar wins
    let moved_movie = item_id(&pool, moved_lib, "movie", "Test Movie").await;
    let sidecar_path = moved.join("Test Movie (2021)/Test Movie (2021).parlour.json");
    let mut edited = read_sidecar(&sidecar_path);
    edited.overview = Some("From the sidecar.".to_string());
    edited.genres = Some(vec!["Thriller".to_string()]);
    edited.runtime_minutes = None; // not carried: the provider's value stays
    edited.vote_average = None;
    std::fs::write(&sidecar_path, sidecar::to_bytes(&edited)).unwrap();
    // A hand-written sidecar naming a record the search would miss.
    let obscure_path = moved.join("Obscure Thing (1984)/Obscure Thing (1984).parlour.json");
    let mut hand = Sidecar::new(sidecar::SidecarKind::Movie);
    hand.title = Some("Obscure Thing Restored".to_string());
    hand.tmdb_id = Some(101);
    hand.genres = Some(vec!["Documentary".to_string()]);
    hand.segments = Some(sidecar::SidecarSegments {
        size_bytes: 12345, // not this encode's size
        entries: vec![sidecar::SidecarSegment {
            kind: "intro".to_string(),
            start_ms: Some(0),
            end_ms: Some(1000),
        }],
    });
    std::fs::write(&obscure_path, sidecar::to_bytes(&hand)).unwrap();
    let report = parlour_catalog::scan_library(&pool, moved_lib, None)
        .await
        .unwrap();
    assert_eq!(report.sidecars_applied, 2);
    assert_eq!(
        report.sidecar_segments_skipped, 1,
        "segments for another encode"
    );
    let obscure = item_id(&pool, moved_lib, "movie", "Obscure Thing").await;
    let o = meta(&pool, obscure).await;
    assert_eq!(o.title, "Obscure Thing Restored");
    assert!(o.manual_segments.is_empty());

    let result = metadata_job(&pool, &settings, moved_lib).await;
    assert_eq!(
        result["matched"], 3,
        "the sidecar's id matched Obscure Thing: {result}"
    );
    assert_eq!(
        result["sidecars_written"], 0,
        "Moved hasn't opted in: {result}"
    );
    let m = meta(&pool, moved_movie).await;
    assert_eq!(m.overview.as_deref(), Some("From the sidecar."));
    assert_eq!(m.genres, vec!["Thriller".to_string()]);
    assert_eq!(
        m.runtime_minutes,
        Some(95),
        "not carried: from the provider"
    );
    let o = meta(&pool, obscure).await;
    assert_eq!(
        (o.title.as_str(), o.tmdb_id, o.overview.as_deref()),
        ("Obscure Thing Restored", Some(101), Some("A test movie.")),
        "the sidecar's id picked the record; its title stands; overview fills in"
    );
    assert_eq!(o.genres, vec!["Documentary".to_string()]);
    let mut expect = moved_before.clone();
    for rel in [
        "Test Movie (2021)/Test Movie (2021).parlour.json",
        "Obscure Thing (1984)/Obscure Thing (1984).parlour.json",
    ] {
        expect.insert(rel.to_string(), tree(&moved).remove(rel).unwrap());
    }
    assert_eq!(
        tree(&moved),
        expect,
        "only the test's own edits changed the folder"
    );

    // ---------------------------------------------------- 5. re-probe keeps the title
    let obscure_video = moved.join("Obscure Thing (1984)/Obscure Thing (1984).mkv");
    let file = std::fs::File::options()
        .write(true)
        .open(&obscure_video)
        .unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
    drop(file);
    let report = parlour_catalog::scan_library(&pool, moved_lib, None)
        .await
        .unwrap();
    assert_eq!((report.probed_files, report.sidecars_applied), (1, 0));
    assert_eq!(
        meta(&pool, obscure).await.title,
        "Obscure Thing Restored",
        "the re-probe's file-name title is replaced by the sidecar's again"
    );

    // ---------------------------------------------------- 6. a bad sidecar
    std::fs::write(
        &obscure_path,
        br#"{"format":"parlour-sidecar","format_version":1,"kind":"movie","title":"Nope","rating":5}"#,
    )
    .unwrap();
    let report = parlour_catalog::scan_library(&pool, moved_lib, None)
        .await
        .unwrap();
    assert_eq!(report.sidecars_applied, 0);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("Obscure Thing (1984).parlour.json") && e.contains("rating")),
        "{:?}",
        report.errors
    );
    assert_eq!(meta(&pool, obscure).await.title, "Obscure Thing Restored");
    // A sidecar of the wrong kind is refused too.
    let mut wrong = Sidecar::new(sidecar::SidecarKind::Episode);
    wrong.title = Some("Nope".to_string());
    std::fs::write(&obscure_path, sidecar::to_bytes(&wrong)).unwrap();
    let report = parlour_catalog::scan_library(&pool, moved_lib, None)
        .await
        .unwrap();
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("kind is \"episode\" but its item's is \"movie\"")),
        "{:?}",
        report.errors
    );
    assert_eq!(meta(&pool, obscure).await.title, "Obscure Thing Restored");

    // ---------------------------------------------------- 7. the admin's fix wins
    // Moved doesn't write: the fix holds over the unchanged sidecar.
    parlour_catalog::apply_manual(&pool, &tmdb, moved_movie, 201)
        .await
        .unwrap();
    assert!(overridden(&pool, moved_movie).await);
    let report = parlour_catalog::scan_library(&pool, moved_lib, None)
        .await
        .unwrap();
    assert_eq!(report.sidecars_applied, 0);
    let m = meta(&pool, moved_movie).await;
    assert_eq!(
        (m.tmdb_id, m.overview.as_deref()),
        (Some(201), Some("One of them.")),
        "a scan doesn't undo the fix while the sidecar file is unchanged"
    );
    // Changing the sidecar file makes it count again.
    edited.overview = Some("Edited again.".to_string());
    std::fs::write(&sidecar_path, sidecar::to_bytes(&edited)).unwrap();
    let report = parlour_catalog::scan_library(&pool, moved_lib, None)
        .await
        .unwrap();
    assert_eq!(report.sidecars_applied, 1);
    assert!(!overridden(&pool, moved_movie).await);
    let m = meta(&pool, moved_movie).await;
    assert_eq!(
        (m.tmdb_id, m.overview.as_deref()),
        (Some(101), Some("Edited again."))
    );

    // Source writes: the fix rewrites the sidecar at once.
    parlour_catalog::apply_manual(&pool, &tmdb, movie, 201)
        .await
        .unwrap();
    let w = parlour_catalog::write_item_sidecars(&pool, movie)
        .await
        .unwrap();
    assert_eq!((w.written, w.errors.len()), (1, 0));
    assert!(!overridden(&pool, movie).await);
    let doc = read_sidecar(&source.join("Test Movie (2021)/Test Movie (2021).parlour.json"));
    assert_eq!(
        (doc.tmdb_id, doc.overview.as_deref()),
        (Some(201), Some("One of them."))
    );
    assert_eq!(
        doc.people.unwrap()[0].name,
        "Bo Player",
        "the new record's cast"
    );
    let report = parlour_catalog::scan_library(&pool, source_lib, None)
        .await
        .unwrap();
    assert_eq!(
        report.sidecars_applied, 0,
        "the rewrite is already recorded"
    );

    // ---------------------------------------------------- 8. a removed sidecar
    std::fs::remove_file(&obscure_path).unwrap();
    let report = parlour_catalog::scan_library(&pool, moved_lib, None)
        .await
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM item_sidecars WHERE item_id = $1")
        .bind(obscure)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "the record goes with the file");
    assert_eq!(
        meta(&pool, obscure).await.title,
        "Obscure Thing Restored",
        "the item keeps its values"
    );

    // The plain library is still untouched after everything.
    assert_eq!(tree(&plain), before);

    let _ = std::fs::remove_dir_all(&root);
}
