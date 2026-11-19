//! End-to-end test of SEG4 (Phase 9, D21): the segment editor.
//!
//! An admin lists an item's stored segments with what each one means
//! for playback, adds and changes the item's manual times — checked
//! only against the file: start before end, both inside it, with no
//! D17/D18 length or position rules, and kinds may overlap — and
//! deletes per D21: deleting a detected segment stores a manual "no
//! segment of this kind here" marker that beats detection and survives
//! a real detection re-run; deleting the marker, or the admin's own
//! times, hands the kind straight back to detection. With the admin's
//! times in place, deleting a detected row is refused rather than
//! throwing them away. Movies are editable; an item with no playable
//! file is not. Every editor route is a 404 for a non-admin.
//!
//! "Preview from here" is the player at a time with no playback
//! reports (web UI); what it asks of the server — playback-info and
//! the stream — writes no watch state, which the flow checks at the
//! end, after many playback-info calls and a stream fetch.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a fresh
//! database of its own) and ffmpeg/ffprobe on PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test segment_editor_flow -- --ignored

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

use reqwest::header::{COOKIE, RANGE, SET_COOKIE};
use reqwest::Method;
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};
use uuid::Uuid;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A tiny real file: a test pattern with a steady tone (no black
/// frames, no silence, so detection finds only what the chapters say).
fn make_media(path: &Path, seconds: u32, chapters: &[(u32, u32, &str)]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg(format!("testsrc=d={seconds}:s=160x120:r=10"))
        .args(["-f", "lavfi", "-i"])
        .arg(format!("sine=f=440:d={seconds}"));
    let meta = path.with_extension("chapters.txt");
    if !chapters.is_empty() {
        let mut text = String::from(";FFMETADATA1\n");
        for (start, end, title) in chapters {
            text.push_str(&format!(
                "[CHAPTER]\nTIMEBASE=1/1000\nSTART={}\nEND={}\ntitle={title}\n",
                start * 1000,
                end * 1000
            ));
        }
        std::fs::write(&meta, text).unwrap();
        cmd.arg("-i")
            .arg(&meta)
            .args(["-map_metadata", "2", "-map_chapters", "2"]);
    }
    cmd.args([
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-shortest",
    ])
    .arg(path);
    let out = cmd.output().expect("ffmpeg available");
    assert!(
        out.status.success(),
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_file(&meta);
}

struct TestApp {
    base: String,
    client: reqwest::Client,
    cookie: String,
}

impl TestApp {
    async fn req(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut builder = self
            .client
            .request(method, format!("{}{}", self.base, path))
            .header(COOKIE, &self.cookie);
        if let Some(body) = body {
            builder = builder.json(&body);
        }
        let resp = builder.send().await.expect("request");
        let status = resp.status().as_u16();
        let value = resp.json::<Value>().await.unwrap_or(Value::Null);
        (status, value)
    }

    async fn get(&self, path: &str) -> Value {
        let (status, value) = self.req(Method::GET, path, None).await;
        assert_eq!(status, 200, "GET {path}: {value}");
        value
    }
}

async fn login(client: &reqwest::Client, base: &str, username: &str, password: &str) -> String {
    let resp = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"username": username, "password": password}))
        .send()
        .await
        .expect("login");
    assert_eq!(resp.status().as_u16(), 200, "login {username}");
    resp.headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

async fn wait_for_job(app: &TestApp, job_id: &str) -> Value {
    for _ in 0..240 {
        let job = app.get(&format!("/api/v1/admin/jobs/{job_id}")).await;
        match job["status"].as_str() {
            Some("succeeded") => return job,
            Some("failed") => panic!("job failed: {job}"),
            _ => sleep(Duration::from_millis(500)).await,
        }
    }
    panic!("job {job_id} did not finish");
}

/// Scan the library and wait for the scan to finish.
async fn scan(app: &TestApp, library_id: &str) {
    let (status, job) = app
        .req(
            Method::POST,
            &format!("/api/v1/admin/libraries/{library_id}/scan"),
            None,
        )
        .await;
    assert_eq!(status, 202, "{job}");
    wait_for_job(app, job["id"].as_str().unwrap()).await;
}

/// The library's detect_segments jobs, newest first.
async fn detect_jobs(app: &TestApp, library_id: &str) -> Vec<Value> {
    let jobs = app
        .get(&format!(
            "/api/v1/admin/jobs?library_id={library_id}&limit=100"
        ))
        .await;
    jobs.as_array()
        .unwrap()
        .iter()
        .filter(|j| j["kind"] == "detect_segments")
        .cloned()
        .collect()
}

type Stored = (String, String, Option<i64>, Option<i64>);

/// Stored rows for an item: (kind, source, start_ms, end_ms) — no
/// times on a marker.
async fn rows(pool: &sqlx::PgPool, item: Uuid) -> Vec<Stored> {
    sqlx::query_as(
        "SELECT kind, source, start_ms, end_ms FROM media_segments
         WHERE item_id = $1 ORDER BY kind DESC, source",
    )
    .bind(item)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn stored(kind: &str, source: &str, start: Option<i64>, end: Option<i64>) -> Stored {
    (kind.into(), source.into(), start, end)
}

/// What playback-info hands the player: (kind, source, start_ms, end_ms).
async fn served(app: &TestApp, item: Uuid) -> Vec<(String, String, i64, i64)> {
    let info = app
        .get(&format!(
            "/api/v1/items/{item}/playback-info?client=web&network=lan"
        ))
        .await;
    info["segments"]
        .as_array()
        .unwrap_or_else(|| panic!("no segments array: {info}"))
        .iter()
        .map(|s| {
            (
                s["kind"].as_str().unwrap().to_string(),
                s["source"].as_str().unwrap().to_string(),
                s["start_ms"].as_i64().unwrap(),
                s["end_ms"].as_i64().unwrap(),
            )
        })
        .collect()
}

fn seg(kind: &str, source: &str, start: i64, end: i64) -> (String, String, i64, i64) {
    (kind.into(), source.into(), start, end)
}

type EditorRow = (String, String, Option<i64>, Option<i64>, String);

/// The editor's rows, in its order: (kind, source, start, end, status).
fn editor_rows(view: &Value) -> Vec<EditorRow> {
    view["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("no rows array: {view}"))
        .iter()
        .map(|r| {
            (
                r["kind"].as_str().unwrap().to_string(),
                r["source"].as_str().unwrap().to_string(),
                r["start_ms"].as_i64(),
                r["end_ms"].as_i64(),
                r["status"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn er(kind: &str, source: &str, start: Option<i64>, end: Option<i64>, status: &str) -> EditorRow {
    (kind.into(), source.into(), start, end, status.into())
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL and ffmpeg"]
async fn segment_editor_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test database url");
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let pool = parlour_db::connect(&db_url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");

    // Fixture library: an episode whose chapters name its intro and
    // credits, an episode with nothing to find, and a 5-minute movie.
    let fixtures = PathBuf::from(format!("/tmp/parlour-segeditor-fixtures-{port}"));
    let _ = std::fs::remove_dir_all(&fixtures);
    let media = fixtures.join("media");
    let season = media.join("Test Show").join("Season 1");
    let ep = |n: u32| season.join(format!("Test Show S01E{n:02}.mkv"));
    make_media(
        &ep(1),
        60,
        &[
            (0, 5, "Prologue"),
            (5, 20, "Intro"),
            (20, 45, "Part A"),
            (45, 60, "End Credits"),
        ],
    );
    make_media(&ep(2), 30, &[]);
    let movie = media
        .join("Movie Night (2020)")
        .join("Movie Night (2020).mkv");
    make_media(&movie, 300, &[]);

    let mut vars = std::collections::HashMap::new();
    vars.insert("PARLOUR_DATABASE_URL".to_string(), db_url.clone());
    vars.insert("PARLOUR_ADMIN_USER".to_string(), "jordan".to_string());
    vars.insert(
        "PARLOUR_ADMIN_PASSWORD".to_string(),
        "admin-pass-123".to_string(),
    );
    let cfg = parlour_server::config::Config::from_vars(&vars).expect("config");
    parlour_server::auth::bootstrap_admin(&pool, &cfg)
        .await
        .expect("bootstrap");

    let mut state = parlour_server::http::AppState::new(pool.clone(), None, false);
    std::sync::Arc::get_mut(&mut state.playback)
        .expect("fresh playback state")
        .configure(fixtures.join("cache"), None);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            parlour_server::http::router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    // The background workers, including segment detection's own.
    parlour_server::catalog::spawn_background(
        pool.clone(),
        parlour_server::metadata::MetadataSettings::default(),
        parlour_catalog::EmbySettings::default(),
    );

    let client = reqwest::Client::new();
    let app = TestApp {
        base: base.clone(),
        client: client.clone(),
        cookie: login(&client, &base, "jordan", "admin-pass-123").await,
    };
    let profiles = app.get("/api/v1/profiles").await;
    let profile_id = profiles[0]["id"].as_str().unwrap().to_string();
    let (status, _) = app
        .req(
            Method::POST,
            &format!("/api/v1/profiles/{profile_id}/select"),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 200);
    let (status, lib) = app
        .req(
            Method::POST,
            "/api/v1/admin/libraries",
            Some(json!({"name": "Shows", "kind": "shows", "paths": [media.to_str().unwrap()]})),
        )
        .await;
    assert_eq!(status, 201, "{lib}");
    let library_id = lib["id"].as_str().unwrap().to_string();
    let (status, _) = app
        .req(
            Method::POST,
            &format!("/api/v1/admin/libraries/{library_id}/access"),
            Some(json!({"profile_id": profile_id})),
        )
        .await;
    assert_eq!(status, 204);

    scan(&app, &library_id).await;
    let jobs = detect_jobs(&app, &library_id).await;
    assert_eq!(jobs.len(), 1, "the scan queues detection: {jobs:?}");
    wait_for_job(&app, jobs[0]["id"].as_str().unwrap()).await;

    let item_of = |path: PathBuf| {
        let pool = pool.clone();
        async move {
            let (id,): (Uuid,) = sqlx::query_as("SELECT item_id FROM media_files WHERE path = $1")
                .bind(path.to_str().unwrap())
                .fetch_one(&pool)
                .await
                .unwrap();
            id
        }
    };
    let (e1, e2, film) = (
        item_of(ep(1)).await,
        item_of(ep(2)).await,
        item_of(movie.clone()).await,
    );
    let (series,): (Uuid,) = sqlx::query_as("SELECT id FROM media_items WHERE kind = 'series'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let editor = |item: Uuid| format!("/api/v1/admin/items/{item}/segments");

    // ---- What detection found, as the editor shows it.
    let view = app.get(&editor(e1)).await;
    assert_eq!(view["item_id"], e1.to_string());
    assert_eq!(
        view["file"]["relative_path"],
        "Test Show/Season 1/Test Show S01E01.mkv"
    );
    let file_ms = view["file"]["duration_ms"].as_i64().unwrap();
    assert!((59_000..=61_000).contains(&file_ms), "{view}");
    assert_eq!(
        editor_rows(&view),
        vec![
            er("intro", "chapters", Some(5_000), Some(20_000), "used"),
            er("credits", "chapters", Some(45_000), Some(60_000), "used"),
        ]
    );
    assert_eq!(
        served(&app, e1).await,
        vec![
            seg("intro", "chapters", 5_000, 20_000),
            seg("credits", "chapters", 45_000, 60_000),
        ]
    );
    let detected = rows(&pool, e1).await;

    // ---- A non-admin gets a 404 from every editor route (A7), and
    // nothing changes.
    let (status, invite) = app
        .req(Method::POST, "/api/v1/admin/invites", Some(json!({})))
        .await;
    assert_eq!(status, 201, "{invite}");
    let resp = client
        .post(format!("{base}/api/v1/invites/accept"))
        .json(&json!({"token": invite["token"], "username": "sam", "password": "sam-pass-123"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 201);
    let sam = TestApp {
        base: base.clone(),
        client: client.clone(),
        cookie: login(&client, &base, "sam", "sam-pass-123").await,
    };
    for (method, path, body) in [
        (Method::GET, editor(e1), None),
        (
            Method::PUT,
            format!("{}/intro", editor(e1)),
            Some(json!({"start_ms": 0, "end_ms": 1_000})),
        ),
        (
            Method::DELETE,
            format!("{}/intro/chapters", editor(e1)),
            None,
        ),
    ] {
        let (status, body) = sam.req(method.clone(), &path, body).await;
        assert_eq!(status, 404, "{method} {path}: {body}");
    }
    assert_eq!(rows(&pool, e1).await, detected);

    // ---- D21: deleting a detected segment stores a manual "no
    // segment of this kind here" marker. The detected row stays —
    // detection owns it — and the player gets no intro.
    let (status, view) = app
        .req(
            Method::DELETE,
            &format!("{}/intro/chapters", editor(e1)),
            None,
        )
        .await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(
        editor_rows(&view),
        vec![
            er("intro", "manual", None, None, "no_segment"),
            er("intro", "chapters", Some(5_000), Some(20_000), "hidden"),
            er("credits", "chapters", Some(45_000), Some(60_000), "used"),
        ]
    );
    assert_eq!(
        rows(&pool, e1).await,
        vec![
            stored("intro", "chapters", Some(5_000), Some(20_000)),
            stored("intro", "manual", None, None),
            stored("credits", "chapters", Some(45_000), Some(60_000)),
        ]
    );
    assert_eq!(
        served(&app, e1).await,
        vec![seg("credits", "chapters", 45_000, 60_000)]
    );
    // Deleting it again changes nothing.
    let (status, again) = app
        .req(
            Method::DELETE,
            &format!("{}/intro/chapters", editor(e1)),
            None,
        )
        .await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(editor_rows(&again), editor_rows(&view));

    // ---- The marker survives a detection re-run: E01 is replaced by a
    // cut whose intro chapter moved, the rescan queues detection, and
    // detection rewrites its own row — the marker still wins.
    make_media(
        &ep(1),
        60,
        &[
            (0, 2, "Prologue"),
            (2, 17, "Intro"),
            (17, 45, "Part A"),
            (45, 60, "End Credits"),
        ],
    );
    scan(&app, &library_id).await;
    let jobs = detect_jobs(&app, &library_id).await;
    assert_eq!(jobs.len(), 2, "the changed file queues detection: {jobs:?}");
    let job = wait_for_job(&app, jobs[0]["id"].as_str().unwrap()).await;
    assert_eq!(job["result"]["episodes"], 1, "{job}");
    assert_eq!(job["result"]["chapter_intros"], 1, "{job}");
    assert_eq!(
        rows(&pool, e1).await,
        vec![
            stored("intro", "chapters", Some(2_000), Some(17_000)),
            stored("intro", "manual", None, None),
            stored("credits", "chapters", Some(45_000), Some(60_000)),
        ]
    );
    assert_eq!(
        served(&app, e1).await,
        vec![seg("credits", "chapters", 45_000, 60_000)]
    );
    // SEG3's pass writes its own source the same way
    // (`fingerprint::write_rows` upserts only `fingerprint` rows); the
    // marker beats that row too.
    sqlx::query(
        "INSERT INTO media_segments (item_id, kind, start_ms, end_ms, source, confidence)
         VALUES ($1, 'intro', 1_500, 16_500, 'fingerprint', 0.95)
         ON CONFLICT (item_id, kind, source) DO UPDATE
         SET start_ms = EXCLUDED.start_ms, end_ms = EXCLUDED.end_ms,
             confidence = EXCLUDED.confidence, updated_at = now()",
    )
    .bind(e1)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        served(&app, e1).await,
        vec![seg("credits", "chapters", 45_000, 60_000)]
    );
    assert_eq!(
        editor_rows(&app.get(&editor(e1)).await),
        vec![
            er("intro", "manual", None, None, "no_segment"),
            er("intro", "chapters", Some(2_000), Some(17_000), "hidden"),
            er("intro", "fingerprint", Some(1_500), Some(16_500), "hidden"),
            er("credits", "chapters", Some(45_000), Some(60_000), "used"),
        ]
    );

    // ---- Deleting the marker hands the intro straight back to
    // detection: the re-run's chapter intro (1.0 beats 0.95).
    let (status, view) = app
        .req(
            Method::DELETE,
            &format!("{}/intro/manual", editor(e1)),
            None,
        )
        .await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(
        editor_rows(&view),
        vec![
            er("intro", "chapters", Some(2_000), Some(17_000), "used"),
            er(
                "intro",
                "fingerprint",
                Some(1_500),
                Some(16_500),
                "outranked"
            ),
            er("credits", "chapters", Some(45_000), Some(60_000), "used"),
        ]
    );
    assert_eq!(
        served(&app, e1).await,
        vec![
            seg("intro", "chapters", 2_000, 17_000),
            seg("credits", "chapters", 45_000, 60_000),
        ]
    );
    let (status, body) = app
        .req(
            Method::DELETE,
            &format!("{}/intro/manual", editor(e1)),
            None,
        )
        .await;
    assert_eq!(status, 404, "no manual intro left: {body}");

    // ---- The admin's own times: added, then changed. Saved as source
    // `manual`, they win over every detector.
    let put_times = |item: Uuid, kind: &'static str, start: i64, end: i64| {
        let path = format!("{}/{kind}", editor(item));
        let app = &app;
        async move {
            app.req(
                Method::PUT,
                &path,
                Some(json!({"start_ms": start, "end_ms": end})),
            )
            .await
        }
    };
    let (status, view) = put_times(e1, "intro", 3_000, 18_000).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(
        editor_rows(&view),
        vec![
            er("intro", "manual", Some(3_000), Some(18_000), "used"),
            er("intro", "chapters", Some(2_000), Some(17_000), "overridden"),
            er(
                "intro",
                "fingerprint",
                Some(1_500),
                Some(16_500),
                "overridden"
            ),
            er("credits", "chapters", Some(45_000), Some(60_000), "used"),
        ]
    );
    let (status, view) = put_times(e1, "intro", 4_000, 19_500).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(
        editor_rows(&view)[0],
        er("intro", "manual", Some(4_000), Some(19_500), "used")
    );
    let (manual_intros,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM media_segments
         WHERE item_id = $1 AND kind = 'intro' AND source = 'manual'",
    )
    .bind(e1)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(manual_intros, 1, "a change updates the one manual row");
    let (confidence,): (f32,) = sqlx::query_as(
        "SELECT confidence FROM media_segments
         WHERE item_id = $1 AND kind = 'intro' AND source = 'manual'",
    )
    .bind(e1)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(confidence, 1.0);
    assert_eq!(
        served(&app, e1).await,
        vec![
            seg("intro", "manual", 4_000, 19_500),
            seg("credits", "chapters", 45_000, 60_000),
        ]
    );
    // With the admin's times in place, deleting a detected intro is
    // refused rather than throwing those times away.
    let before = rows(&pool, e1).await;
    let (status, body) = app
        .req(
            Method::DELETE,
            &format!("{}/intro/fingerprint", editor(e1)),
            None,
        )
        .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(rows(&pool, e1).await, before);
    // Deleting the admin's times: detection decides again.
    let (status, view) = app
        .req(
            Method::DELETE,
            &format!("{}/intro/manual", editor(e1)),
            None,
        )
        .await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(
        served(&app, e1).await,
        vec![
            seg("intro", "chapters", 2_000, 17_000),
            seg("credits", "chapters", 45_000, 60_000),
        ]
    );

    // ---- D21's only checks on manual times: start before end, and
    // both inside the file.
    for (start, end) in [
        (5_000, 5_000),
        (6_000, 5_000),
        (-1, 1_000),
        (0, file_ms + 1),
    ] {
        let (status, body) = put_times(e1, "credits", start, end).await;
        assert_eq!(status, 400, "{start}..{end}: {body}");
    }
    let (_, body) = put_times(e1, "credits", 0, file_ms + 1).await;
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("past the end of the file"),
        "{body}"
    );
    assert_eq!(
        rows(&pool, e1).await,
        vec![
            stored("intro", "chapters", Some(2_000), Some(17_000)),
            stored("intro", "fingerprint", Some(1_500), Some(16_500)),
            stored("credits", "chapters", Some(45_000), Some(60_000)),
        ],
        "nothing was stored"
    );
    // Everything else is the admin's call: a recap over the whole file
    // (the end may be the file's very end) and 2 s of "credits" in the
    // middle of the intro — kinds overlap, and D17/D18's lengths and
    // positions don't apply.
    let (status, body) = put_times(e1, "recap", 0, file_ms).await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = put_times(e1, "credits", 10_000, 12_000).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        served(&app, e1).await,
        vec![
            seg("recap", "manual", 0, file_ms),
            seg("intro", "chapters", 2_000, 17_000),
            seg("credits", "manual", 10_000, 12_000),
        ]
    );
    let view = app.get(&editor(e1)).await;
    assert_eq!(
        editor_rows(&view),
        vec![
            er("recap", "manual", Some(0), Some(file_ms), "used"),
            er("intro", "chapters", Some(2_000), Some(17_000), "used"),
            er(
                "intro",
                "fingerprint",
                Some(1_500),
                Some(16_500),
                "outranked"
            ),
            er("credits", "manual", Some(10_000), Some(12_000), "used"),
            er(
                "credits",
                "chapters",
                Some(45_000),
                Some(60_000),
                "overridden"
            ),
        ]
    );

    // ---- Requests that name nothing real.
    let (status, body) = put_times(e1, "outro", 0, 1_000).await;
    assert_eq!(status, 400, "unknown kind: {body}");
    let (status, body) = app
        .req(Method::DELETE, &format!("{}/intro/guess", editor(e1)), None)
        .await;
    assert_eq!(status, 400, "unknown source: {body}");
    let (status, body) = app
        .req(
            Method::DELETE,
            &format!("{}/recap/chapters", editor(e1)),
            None,
        )
        .await;
    assert_eq!(status, 404, "no detected recap to delete: {body}");
    let (status, _) = app
        .req(
            Method::PUT,
            &format!("{}/intro", editor(e1)),
            Some(json!({"start_ms": 0})),
        )
        .await;
    assert!((400..500).contains(&status), "an end is required: {status}");
    let nobody = Uuid::new_v4();
    for (method, path) in [
        (Method::GET, editor(nobody)),
        (Method::PUT, format!("{}/intro", editor(nobody))),
        (Method::DELETE, format!("{}/intro/manual", editor(nobody))),
        (
            Method::GET,
            "/api/v1/admin/items/not-a-uuid/segments".into(),
        ),
    ] {
        let (status, body) = app
            .req(
                method.clone(),
                &path,
                Some(json!({"start_ms": 0, "end_ms": 1_000})),
            )
            .await;
        assert_eq!(status, 404, "{method} {path}: {body}");
    }

    // ---- A movie: detection never looks at it, but its segments are
    // the admin's to add, and they reach the player. 200–205 s starts
    // past D17's 120 s head bound and is under D18's 15 s minimum;
    // neither applies to an admin's times.
    let view = app.get(&editor(film)).await;
    assert_eq!(
        view["file"]["relative_path"],
        "Movie Night (2020)/Movie Night (2020).mkv"
    );
    assert_eq!(editor_rows(&view), vec![]);
    let (status, view) = put_times(film, "intro", 200_000, 205_000).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(
        served(&app, film).await,
        vec![seg("intro", "manual", 200_000, 205_000)]
    );

    // ---- No playable file, no segments: a series, and an episode
    // whose only file has gone missing (C8 keeps its row).
    let view = app.get(&editor(series)).await;
    assert_eq!(view["file"], Value::Null, "{view}");
    assert_eq!(editor_rows(&view), vec![]);
    let (status, body) = put_times(series, "intro", 0, 1_000).await;
    assert_eq!(status, 400, "{body}");
    sqlx::query("UPDATE media_files SET available = false WHERE item_id = $1")
        .bind(e2)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(app.get(&editor(e2)).await["file"], Value::Null);
    let (status, body) = put_times(e2, "intro", 0, 1_000).await;
    assert_eq!(status, 400, "{body}");
    sqlx::query("UPDATE media_files SET available = true WHERE item_id = $1")
        .bind(e2)
        .execute(&pool)
        .await
        .unwrap();
    assert!(rows(&pool, e2).await.is_empty(), "E02 was never touched");

    // ---- The schema behind the marker (migration
    // 20261119000000): only a manual row may have no times, it has
    // neither time then, and an item holds the admin's times or the
    // admin's "none" for a kind — never both.
    let insert = |start: Option<i64>, end: Option<i64>, source: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO media_segments (item_id, kind, start_ms, end_ms, source, confidence)
                 VALUES ($1, 'intro', $2, $3, $4, 1.0)",
            )
            .bind(e2)
            .bind(start)
            .bind(end)
            .bind(source)
            .execute(&pool)
            .await
        }
    };
    for source in ["chapters", "fingerprint", "heuristic"] {
        assert!(
            insert(None, None, source).await.is_err(),
            "a detector's row always has times ({source})"
        );
    }
    assert!(insert(None, Some(1_000), "manual").await.is_err());
    assert!(insert(Some(0), None, "manual").await.is_err());
    insert(None, None, "manual").await.unwrap();
    assert!(
        insert(Some(0), Some(1_000), "manual").await.is_err(),
        "times and a marker for the same kind"
    );
    sqlx::query("DELETE FROM media_segments WHERE item_id = $1")
        .bind(e2)
        .execute(&pool)
        .await
        .unwrap();

    // ---- "Preview from here" asks the server only for playback-info
    // and the stream. Neither writes watch state: after every
    // playback-info call above and a ranged stream read, the admin's
    // profile has no position, played mark or play count, no live
    // session, and no playback activity.
    let info = app
        .get(&format!(
            "/api/v1/items/{e2}/playback-info?client=web&network=lan"
        ))
        .await;
    let file_id = info["file_id"].as_str().unwrap();
    let resp = client
        .get(format!("{base}/api/v1/stream/{file_id}"))
        .header(COOKIE, &app.cookie)
        .header(RANGE, "bytes=0-1023")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 206);
    let (user_rows,): (i64,) = sqlx::query_as("SELECT count(*) FROM user_data")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(user_rows, 0, "no watch state was written");
    assert_eq!(app.get("/api/v1/admin/sessions").await, json!([]));
    let activity = app.get("/api/v1/admin/activity?category=playback").await;
    assert_eq!(activity["entries"], json!([]), "{activity}");

    server.abort();
    let _ = std::fs::remove_dir_all(&fixtures);
}
