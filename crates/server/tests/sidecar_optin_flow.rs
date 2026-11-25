//! The sidecar write opt-in (XFER5, D14), end to end over HTTP against
//! a real Postgres and a **stub TMDB server** (no key, no internet),
//! with real ffmpeg-made files in a temp folder.
//!
//! Covers:
//! - a new library has `write_sidecars` off, and creating or editing a
//!   library can't set it — only its own endpoint can (non-video
//!   libraries can't turn it on); turning it on or off is in Activity;
//! - with it off, the metadata job and the artwork job write nothing
//!   next to the media;
//! - turned on, the metadata job writes the sidecars and the artwork
//!   job copies `poster.jpg` / `backdrop.jpg` into folders that belong
//!   to one item — not a library root, not a folder two movies share,
//!   and never over a `poster.jpg` Parlour didn't write;
//! - re-running both jobs touches nothing; changed cached artwork
//!   refreshes Parlour's own copies only; the scanner ignores them;
//! - a folder Parlour can't write fails cleanly: the job succeeds and
//!   lists each failed path, old files stay, no temp files are left;
//!   the library view says which roots aren't writable and a notice
//!   stays up until the root is writable (or the opt-in is off);
//! - turned off again, nothing more is written.
//!
//! Ignored by default: set PARLOUR_TEST_DATABASE_URL (a fresh
//! database of its own).

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::SystemTime;

use axum::extract::{Path as AxumPath, Query};
use axum::routing::get;
use axum::{Json, Router};
use parlour_server::config::Config;
use parlour_server::http::AppState;
use serde_json::{json, Value};
use uuid::Uuid;

// ------------------------------------------------------------ stub TMDB

/// Which "version" of the artwork the stub serves; bumping it is a
/// provider changing its images.
static ART_GENERATION: AtomicU8 = AtomicU8::new(1);

const MOVIES: &[(i64, &str, &str)] = &[
    (101, "Test Movie", "2021-05-01"),
    (102, "Second Film", "2019-02-02"),
    (103, "Third Film", "2018-03-03"),
    (104, "Root Film", "2017-04-04"),
    (105, "Foreign Film", "2016-05-05"),
    (106, "Fresh Film", "2015-06-06"),
];

async fn stub_search_movie(Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let query = q.get("query").cloned().unwrap_or_default();
    let results: Vec<Value> = MOVIES
        .iter()
        .filter(|(_, title, _)| *title == query)
        .map(|(id, title, date)| json!({"id": id, "title": title, "release_date": date}))
        .collect();
    Json(json!({ "results": results }))
}

async fn stub_search_tv(Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let results = match q.get("query").map(String::as_str) {
        Some("Test Show") => json!([
            {"id": 301, "name": "Test Show", "first_air_date": "2020-03-01"}
        ]),
        _ => json!([]),
    };
    Json(json!({ "results": results }))
}

async fn stub_movie(AxumPath(id): AxumPath<i64>) -> Json<Value> {
    let (_, title, date) = MOVIES
        .iter()
        .find(|(m, _, _)| *m == id)
        .copied()
        .unwrap_or((id, "Stub", "2000-01-01"));
    Json(json!({
        "id": id, "title": title, "release_date": date,
        "overview": format!("About {title}."), "runtime": 90, "vote_average": 7.0,
        "genres": [{"name": "Drama"}],
        "poster_path": format!("/poster{id}.jpg"),
        "backdrop_path": format!("/backdrop{id}.jpg"),
        "external_ids": {}, "release_dates": {"results": []},
        "credits": {"cast": []}
    }))
}

async fn stub_tv(AxumPath(id): AxumPath<i64>) -> Json<Value> {
    Json(json!({
        "id": id, "name": "Test Show", "first_air_date": "2020-03-01",
        "overview": "A show.", "episode_run_time": [42], "vote_average": 8.1,
        "genres": [{"name": "Sci-Fi"}],
        "poster_path": format!("/poster{id}.jpg"),
        "backdrop_path": format!("/backdrop{id}.jpg"),
        "external_ids": {}, "content_ratings": {"results": []},
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

/// The JPEG the stub serves for one image file at one generation.
fn jpeg(file: &str, generation: u8) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0];
    bytes.extend_from_slice(file.as_bytes());
    bytes.push(generation);
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

async fn stub_image(AxumPath((size, file)): AxumPath<(String, String)>) -> Vec<u8> {
    let _ = size;
    jpeg(&file, ART_GENERATION.load(Ordering::SeqCst))
}

fn stub_app() -> Router {
    Router::new()
        .route("/search/movie", get(stub_search_movie))
        .route("/search/tv", get(stub_search_tv))
        .route("/movie/{id}", get(stub_movie))
        .route("/tv/{id}", get(stub_tv))
        .route("/tv/{id}/season/{n}", get(stub_season))
        .route("/images/{size}/{file}", get(stub_image))
}

// ------------------------------------------------------------ helpers

fn test_config(url: &str) -> Config {
    let mut vars = HashMap::new();
    vars.insert("PARLOUR_DATABASE_URL".to_string(), url.to_string());
    vars.insert("PARLOUR_ADMIN_USER".to_string(), "jordan".to_string());
    vars.insert(
        "PARLOUR_ADMIN_PASSWORD".to_string(),
        "admin-pass-123".to_string(),
    );
    Config::from_vars(&vars).expect("config")
}

struct Client {
    http: reqwest::Client,
    base: String,
    cookie: String,
}

impl Client {
    async fn req(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> (reqwest::StatusCode, Value) {
        let mut rb = self
            .http
            .request(method, format!("{}{path}", self.base))
            .header("Cookie", format!("parlour_session={}", self.cookie));
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let res = rb.send().await.expect("request");
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// Start a job with a POST and wait for it; it must succeed.
    async fn run_job(&self, path: &str) -> Value {
        let (s, job) = self.req(reqwest::Method::POST, path, None).await;
        assert_eq!(s, 202, "{path}: {job}");
        let id = job["id"].as_str().unwrap().to_string();
        for _ in 0..120 {
            let (_, job) = self
                .req(
                    reqwest::Method::GET,
                    &format!("/api/v1/admin/jobs/{id}"),
                    None,
                )
                .await;
            match job["status"].as_str() {
                Some("succeeded") => return job,
                Some("failed") => panic!("{path}: job failed: {job}"),
                _ => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
            }
        }
        panic!("{path}: job did not finish");
    }

    async fn set_flag(&self, library: &str, enabled: bool) -> (reqwest::StatusCode, Value) {
        self.req(
            reqwest::Method::PUT,
            &format!("/api/v1/admin/libraries/{library}/write-sidecars"),
            Some(json!({ "enabled": enabled })),
        )
        .await
    }
}

async fn login(http: &reqwest::Client, base: &str) -> (String, Value) {
    let res = http
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"username": "jordan", "password": "admin-pass-123"}))
        .send()
        .await
        .expect("login");
    assert_eq!(res.status(), 200);
    let cookie = res
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.split(';')
                .next()
                .and_then(|kv| kv.split_once('='))
                .map(|(_, v)| v.to_string())
        })
        .expect("session cookie");
    (cookie, res.json().await.unwrap())
}

fn make_media(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let ok = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg("testsrc=duration=2:size=160x120:rate=10")
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2"])
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

/// Files Parlour may write: sidecars, artwork copies, temp files.
fn written(root: &Path) -> Vec<String> {
    tree(root)
        .into_keys()
        .filter(|n| {
            n.ends_with(".parlour.json")
                || n.ends_with("poster.jpg")
                || n.ends_with("backdrop.jpg")
                || n.ends_with(".tmp")
        })
        .collect()
}

fn chmod(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

async fn flag(pool: &parlour_db::PgPool, library: &str) -> bool {
    let (b,): (bool,) = sqlx::query_as("SELECT write_sidecars FROM libraries WHERE id = $1::uuid")
        .bind(library)
        .fetch_one(pool)
        .await
        .unwrap();
    b
}

async fn library_activity(client: &Client) -> Vec<String> {
    let (s, page) = client
        .req(
            reqwest::Method::GET,
            "/api/v1/admin/activity?category=library",
            None,
        )
        .await;
    assert_eq!(s, 200, "{page}");
    page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_string())
        .collect()
}

async fn unwritable_notices(client: &Client) -> Vec<Value> {
    let (s, list) = client
        .req(reqwest::Method::GET, "/api/v1/admin/notices", None)
        .await;
    assert_eq!(s, 200, "{list}");
    list.as_array()
        .unwrap()
        .iter()
        .filter(|n| n["kind"] == "sidecars_unwritable")
        .cloned()
        .collect()
}

fn job_errors(job: &Value) -> Vec<String> {
    job["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["message"].as_str().unwrap_or_default().to_string())
        .collect()
}

// ------------------------------------------------------------ the flow

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn sidecar_optin_flow_end_to_end() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let config = test_config(&db_url);
    let pool = parlour_db::connect(&config.database_url).await.unwrap();
    parlour_db::migrate(&pool).await.unwrap();
    parlour_server::auth::bootstrap_admin(&pool, &config)
        .await
        .unwrap();

    let stub_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stub_addr = stub_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(stub_listener, stub_app()).await.unwrap();
    });

    let root: PathBuf = std::env::temp_dir().join(format!(
        "parlour-sidecar-optin-flow-{}",
        Uuid::new_v4().simple()
    ));
    let media = root.join("media");
    make_media(&media.join("Test Movie (2021)/Test Movie (2021).mkv"));
    make_media(&media.join("Test Show/Season 1/Test Show - S01E01.mkv"));
    // Two movies in one folder: a single poster.jpg can't say whose.
    make_media(&media.join("Duo/Second Film (2019).mkv"));
    make_media(&media.join("Duo/Third Film (2018).mkv"));
    // A movie straight in the library root.
    make_media(&media.join("Root Film (2017).mkv"));
    // A folder that already has someone else's poster.
    make_media(&media.join("Foreign Film (2016)/Foreign Film (2016).mkv"));
    let foreign_poster = media.join("Foreign Film (2016)/poster.jpg");
    std::fs::write(&foreign_poster, b"a poster another program saved").unwrap();

    let mut state = AppState::new(pool.clone(), None, false);
    state.metadata = parlour_server::metadata::MetadataSettings {
        tmdb_api_key: Some("test-key".to_string()),
        tmdb_base: Some(format!("http://{stub_addr}")),
        artwork_dir: root.join("artwork"),
        ..Default::default()
    };
    let app = parlour_server::http::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (cookie, me) = login(&http, &base).await;
    let client = Client { http, base, cookie };
    let profile = me["profiles"][0]["id"].as_str().unwrap().to_string();
    let (s, _) = client
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/profiles/{profile}/select"),
            Some(json!({})),
        )
        .await;
    assert_eq!(s, 200);

    // ---------------------------------------------------- 1. off by default
    let media_str = media.to_string_lossy().to_string();
    let (s, refused) = client
        .req(
            reqwest::Method::POST,
            "/api/v1/admin/libraries",
            Some(
                json!({"name": "Films", "kind": "movies", "paths": [media_str],
                        "write_sidecars": true}),
            ),
        )
        .await;
    assert_eq!(
        s, 400,
        "creating a library can't turn the opt-in on: {refused}"
    );
    let (s, lib) = client
        .req(
            reqwest::Method::POST,
            "/api/v1/admin/libraries",
            Some(json!({"name": "Films", "kind": "movies", "paths": [media_str]})),
        )
        .await;
    assert_eq!(s, 201, "{lib}");
    assert_eq!(lib["write_sidecars"], false, "off by default");
    assert_eq!(
        lib["folders"],
        json!([{"path": media_str, "writable": true, "problem": null}])
    );
    let library = lib["id"].as_str().unwrap().to_string();
    let (s, _) = client
        .req(
            reqwest::Method::PATCH,
            &format!("/api/v1/admin/libraries/{library}"),
            Some(json!({"name": "Films", "write_sidecars": true})),
        )
        .await;
    assert_eq!(s, 400, "editing a library can't turn the opt-in on");
    assert!(!flag(&pool, &library).await);
    // Only video libraries take it.
    let (_, books) = client
        .req(
            reqwest::Method::POST,
            "/api/v1/admin/libraries",
            Some(json!({"name": "Books", "kind": "audiobooks", "paths": []})),
        )
        .await;
    let (s, _) = client.set_flag(books["id"].as_str().unwrap(), true).await;
    assert_eq!(s, 400, "an audiobooks library can't write sidecars yet");
    // The body is strict.
    let (s, _) = client
        .req(
            reqwest::Method::PUT,
            &format!("/api/v1/admin/libraries/{library}/write-sidecars"),
            Some(json!({"enabled": true, "also": 1})),
        )
        .await;
    assert!(s.is_client_error(), "{s}");
    assert!(!flag(&pool, &library).await);

    // ---------------------------------------------------- 2. off: nothing written
    let library_id = Uuid::parse_str(&library).unwrap();
    let scan = parlour_catalog::scan_library(&pool, library_id, None)
        .await
        .unwrap();
    assert_eq!(scan.seen_files, 6);
    assert!(scan.errors.is_empty(), "{:?}", scan.errors);
    let before = tree(&media);
    let meta = client
        .run_job(&format!("/api/v1/admin/libraries/{library}/metadata"))
        .await;
    assert_eq!(meta["result"]["matched"], 6, "{meta}");
    assert_eq!(meta["result"]["sidecars_written"], 0);
    let art = client
        .run_job(&format!("/api/v1/admin/libraries/{library}/artwork"))
        .await;
    assert_eq!(art["result"]["copied"], 0, "{art}");
    assert!(
        art["result"]["downloaded"].as_i64().unwrap() > 0,
        "the cache still fills: {art}"
    );
    assert_eq!(
        tree(&media),
        before,
        "with the opt-in off nothing is written, renamed or touched"
    );

    // ---------------------------------------------------- 3. turned on
    let (s, on) = client.set_flag(&library, true).await;
    assert_eq!(s, 200, "{on}");
    assert_eq!(on["write_sidecars"], true);
    let (s, _) = client.set_flag(&library, true).await;
    assert_eq!(s, 200, "turning it on twice is fine");
    assert_eq!(
        library_activity(&client).await,
        vec!["library.sidecars_on".to_string()],
        "recorded once, by the admin"
    );

    let meta = client
        .run_job(&format!("/api/v1/admin/libraries/{library}/metadata"))
        .await;
    assert!(job_errors(&meta).is_empty(), "{meta}");
    // Five movie files' sidecars, the episode's, the series folder's.
    assert_eq!(meta["result"]["sidecars_written"], 7, "{meta}");
    let art = client
        .run_job(&format!("/api/v1/admin/libraries/{library}/artwork"))
        .await;
    assert!(job_errors(&art).is_empty(), "{art}");
    assert_eq!(
        art["result"]["copied"], 5,
        "movie folder 2, series folder 2, the foreign folder's backdrop: {art}"
    );
    let notes = art["result"]["copies_left_alone"].to_string();
    assert!(
        notes.contains("Foreign Film (2016)/poster.jpg"),
        "the left-alone poster is reported: {notes}"
    );
    let mut expected = vec![
        "Duo/Second Film (2019).parlour.json",
        "Duo/Third Film (2018).parlour.json",
        "Foreign Film (2016)/Foreign Film (2016).parlour.json",
        "Foreign Film (2016)/backdrop.jpg",
        "Foreign Film (2016)/poster.jpg",
        "Root Film (2017).parlour.json",
        "Test Movie (2021)/Test Movie (2021).parlour.json",
        "Test Movie (2021)/backdrop.jpg",
        "Test Movie (2021)/poster.jpg",
        "Test Show/Season 1/Test Show - S01E01.parlour.json",
        "Test Show/backdrop.jpg",
        "Test Show/poster.jpg",
        "Test Show/series.parlour.json",
    ];
    expected.sort();
    assert_eq!(written(&media), expected, "no temp files, nothing else");
    assert_eq!(
        std::fs::read(media.join("Test Movie (2021)/poster.jpg")).unwrap(),
        jpeg("poster101.jpg", 1)
    );
    assert_eq!(
        std::fs::read(media.join("Test Show/backdrop.jpg")).unwrap(),
        jpeg("backdrop301.jpg", 1)
    );
    assert_eq!(
        std::fs::read(&foreign_poster).unwrap(),
        b"a poster another program saved",
        "a poster Parlour didn't write is never replaced"
    );
    let (rows,): (i64,) = sqlx::query_as("SELECT count(*) FROM sidecar_artwork")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 5);

    // ---------------------------------------------------- 4. re-runs touch nothing
    let settled = tree(&media);
    let meta = client
        .run_job(&format!("/api/v1/admin/libraries/{library}/metadata"))
        .await;
    assert_eq!(meta["result"]["sidecars_written"], 0, "{meta}");
    let art = client
        .run_job(&format!("/api/v1/admin/libraries/{library}/artwork"))
        .await;
    assert_eq!(art["result"]["copied"], 0, "{art}");
    assert_eq!(
        tree(&media),
        settled,
        "same bytes: every file left alone (mtimes too)"
    );
    // The scanner ignores the copies: no new items, no errors.
    let rescan = parlour_catalog::scan_library(&pool, library_id, None)
        .await
        .unwrap();
    assert_eq!((rescan.seen_files, rescan.created_items), (6, 0));
    assert!(rescan.errors.is_empty(), "{:?}", rescan.errors);

    // ---------------------------------------------------- 5. new artwork refreshes our copies
    ART_GENERATION.store(2, Ordering::SeqCst);
    client
        .run_job(&format!("/api/v1/admin/libraries/{library}/metadata"))
        .await;
    let art = client
        .run_job(&format!("/api/v1/admin/libraries/{library}/artwork"))
        .await;
    assert_eq!(art["result"]["copied"], 5, "{art}");
    assert_eq!(
        std::fs::read(media.join("Test Movie (2021)/poster.jpg")).unwrap(),
        jpeg("poster101.jpg", 2)
    );
    assert_eq!(
        std::fs::read(media.join("Foreign Film (2016)/backdrop.jpg")).unwrap(),
        jpeg("backdrop105.jpg", 2)
    );
    assert_eq!(
        std::fs::read(&foreign_poster).unwrap(),
        b"a poster another program saved"
    );
    // Someone replaces one of Parlour's copies: from then on it's theirs.
    std::fs::write(media.join("Test Show/poster.jpg"), b"hand-picked").unwrap();
    ART_GENERATION.store(3, Ordering::SeqCst);
    client
        .run_job(&format!("/api/v1/admin/libraries/{library}/metadata"))
        .await;
    let art = client
        .run_job(&format!("/api/v1/admin/libraries/{library}/artwork"))
        .await;
    assert_eq!(art["result"]["copied"], 4, "{art}");
    assert_eq!(
        std::fs::read(media.join("Test Show/poster.jpg")).unwrap(),
        b"hand-picked"
    );

    // ---------------------------------------------------- 6. a folder Parlour can't write
    let fresh = media.join("Fresh Film (2015)");
    make_media(&fresh.join("Fresh Film (2015).mkv"));
    chmod(&fresh, 0o555);
    let enforced = std::fs::File::create(fresh.join("probe")).is_err();
    if enforced {
        parlour_catalog::scan_library(&pool, library_id, None)
            .await
            .unwrap();
        ART_GENERATION.store(4, Ordering::SeqCst);
        let meta = client
            .run_job(&format!("/api/v1/admin/libraries/{library}/metadata"))
            .await;
        let errors = job_errors(&meta);
        // The sidecar and, since XFER6, the NFO beside it.
        assert!(
            errors.len() == 2
                && errors[0].contains("Fresh Film (2015)/Fresh Film (2015).parlour.json")
                && errors[1].contains("Fresh Film (2015)/Fresh Film (2015).nfo"),
            "the failed sidecar and NFO are listed on a job that still succeeds: {errors:?}"
        );
        let art = client
            .run_job(&format!("/api/v1/admin/libraries/{library}/artwork"))
            .await;
        let errors = job_errors(&art);
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors
            .iter()
            .any(|e| e.contains("Fresh Film (2015)/poster.jpg")));
        assert!(errors
            .iter()
            .any(|e| e.contains("Fresh Film (2015)/backdrop.jpg")));
        assert_eq!(
            art["result"]["copied"], 4,
            "the rest carried on (the hand-picked poster stays): {art}"
        );
        assert_eq!(
            tree(&fresh).into_keys().collect::<Vec<_>>(),
            vec!["Fresh Film (2015).mkv".to_string()],
            "nothing half-written, no temp file"
        );

        // A root that can't be written: the library says so, and a
        // notice stays up until it can.
        chmod(&media, 0o555);
        let (_, detail) = client
            .req(
                reqwest::Method::GET,
                &format!("/api/v1/admin/libraries/{library}"),
                None,
            )
            .await;
        assert_eq!(detail["folders"][0]["writable"], false, "{detail}");
        assert!(detail["folders"][0]["problem"].is_string());
        parlour_server::notices::reconcile_with(&pool, &[])
            .await
            .unwrap();
        let notices = unwritable_notices(&client).await;
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0]["summary"].as_str().unwrap().contains("Films"),
            "{notices:?}"
        );
        chmod(&media, 0o755);
        parlour_server::notices::reconcile_with(&pool, &[])
            .await
            .unwrap();
        assert!(unwritable_notices(&client).await.is_empty(), "resolved");

        // Off again while the root is read-only: no notice either.
        chmod(&media, 0o555);
        let (s, off) = client.set_flag(&library, false).await;
        assert_eq!(s, 200, "{off}");
        parlour_server::notices::reconcile_with(&pool, &[])
            .await
            .unwrap();
        assert!(unwritable_notices(&client).await.is_empty());
        chmod(&media, 0o755);
    } else {
        eprintln!("running as root: folder permissions don't bind, read-only part skipped");
        let (s, _) = client.set_flag(&library, false).await;
        assert_eq!(s, 200);
    }
    chmod(&fresh, 0o755);

    // ---------------------------------------------------- 7. off again: nothing more written
    assert!(!flag(&pool, &library).await);
    assert_eq!(
        library_activity(&client).await,
        vec![
            "library.sidecars_off".to_string(),
            "library.sidecars_on".to_string()
        ]
    );
    let quiet = tree(&media);
    ART_GENERATION.store(5, Ordering::SeqCst);
    let meta = client
        .run_job(&format!("/api/v1/admin/libraries/{library}/metadata"))
        .await;
    assert_eq!(meta["result"]["sidecars_written"], 0);
    let art = client
        .run_job(&format!("/api/v1/admin/libraries/{library}/artwork"))
        .await;
    assert_eq!(art["result"]["copied"], 0, "{art}");
    assert!(job_errors(&art).is_empty(), "{art}");
    assert_eq!(
        tree(&media),
        quiet,
        "turned off: nothing written or touched"
    );

    std::fs::remove_dir_all(&root).unwrap();
}
