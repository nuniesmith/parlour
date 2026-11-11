//! End-to-end catalog flow against a real Postgres (C1–C8).
//!
//! Ignored by default: set PARLOUR_TEST_DATABASE_URL to a
//! throwaway database and run `cargo test -- --ignored`.
//! Fixture media is generated with the host's ffmpeg (the same
//! tool the scanner probes with), in a temp folder.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

use parlour_server::config::Config;
use parlour_server::http::AppState;
use serde_json::{json, Value};

fn test_config(url: &str) -> Config {
    let mut vars = std::collections::HashMap::new();
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
}

impl Client {
    async fn req(
        &self,
        method: reqwest::Method,
        path: &str,
        cookie: Option<&str>,
        body: Option<Value>,
    ) -> (reqwest::StatusCode, Value) {
        let mut rb = self.http.request(method, format!("{}{path}", self.base));
        if let Some(c) = cookie {
            rb = rb.header("Cookie", format!("parlour_session={c}"));
        }
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let res = rb.send().await.expect("request");
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        (status, value)
    }

    async fn login_full(&self, username: &str, password: &str) -> (String, Value) {
        let mut rb = self
            .http
            .request(
                reqwest::Method::POST,
                format!("{}/api/v1/auth/login", self.base),
            )
            .json(&json!({"username": username, "password": password}));
        let _ = &mut rb;
        let res = rb.send().await.expect("login");
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
        let body: Value = res.json().await.unwrap();
        (cookie, body)
    }
}

/// Generate a tiny real media file (optionally with an embedded
/// English subtitle) with ffmpeg.
fn make_media(path: &Path, with_subs: bool, srt: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    if with_subs {
        std::fs::write(
            srt,
            "1\n00:00:00,000 --> 00:00:01,500\nHello from the fixture.\n",
        )
        .unwrap();
    }
    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-y",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc=duration=2:size=320x240:rate=10",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=2",
    ]);
    if with_subs {
        cmd.arg("-i").arg(srt);
    }
    cmd.args(["-map", "0:v", "-map", "1:a"]);
    if with_subs {
        cmd.args([
            "-map",
            "2:s",
            "-c:s",
            "srt",
            "-metadata:s:s:0",
            "language=eng",
        ]);
    }
    cmd.args([
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-shortest",
    ])
    .arg(path);
    let out = cmd.output().expect("ffmpeg runs");
    assert!(
        out.status.success(),
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

async fn scan_and_wait(client: &Client, admin: &str, library_id: &str) -> Value {
    let (status, job) = client
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/admin/libraries/{library_id}/scan"),
            Some(admin),
            None,
        )
        .await;
    assert_eq!(status, 202, "{job}");
    let job_id = job["id"].as_str().unwrap().to_string();
    for _ in 0..60 {
        let (status, job) = client
            .req(
                reqwest::Method::GET,
                &format!("/api/v1/admin/jobs/{job_id}"),
                Some(admin),
                None,
            )
            .await;
        assert_eq!(status, 200);
        if job["status"] == "succeeded" {
            return job;
        }
        assert_ne!(job["status"], "failed", "{job}");
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("scan job did not finish");
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn full_catalog_flow() {
    let url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let cfg = test_config(&url);
    let pool = parlour_db::connect(&url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");
    parlour_server::auth::bootstrap_admin(&pool, &cfg)
        .await
        .expect("bootstrap");

    let state = AppState::new(pool, None, false);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            parlour_server::http::router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = Client {
        http: reqwest::Client::new(),
        base: format!("http://{addr}"),
    };
    use reqwest::Method;

    // Fixture media tree.
    let root: PathBuf =
        std::env::temp_dir().join(format!("parlour-catalog-{}", std::process::id()));
    let movies = root.join("movies");
    let shows = root.join("shows");
    let srt = root.join("fixture.srt");
    let movie_file = movies
        .join("Test Movie (2021)")
        .join("Test Movie (2021).mkv");
    make_media(&movie_file, true, &srt);
    // Sidecar subtitle for the same movie.
    std::fs::write(
        movies
            .join("Test Movie (2021)")
            .join("Test Movie (2021).en.srt"),
        "1\n00:00:00,000 --> 00:00:01,500\nSidecar line.\n",
    )
    .unwrap();
    // A sample that must be skipped.
    let sample = movies
        .join("Test Movie (2021)")
        .join("Sample")
        .join("sample.mkv");
    make_media(&sample, false, &srt);
    // One episode.
    let episode = shows
        .join("Test Show (2022)")
        .join("Season 01")
        .join("Test Show - S01E01 - Pilot.mkv");
    make_media(&episode, false, &srt);

    // Admin + profile selection.
    let (admin, login) = client.login_full("jordan", "admin-pass-123").await;
    let profile_id = login["profiles"][0]["id"].as_str().unwrap().to_string();
    let (status, _) = client
        .req(
            Method::POST,
            &format!("/api/v1/profiles/{profile_id}/select"),
            Some(&admin),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 200);

    // C2: libraries with root paths.
    let (status, movies_lib) = client
        .req(
            Method::POST,
            "/api/v1/admin/libraries",
            Some(&admin),
            Some(json!({"name": "Movies", "kind": "movies", "paths": [movies.to_string_lossy()] })),
        )
        .await;
    assert_eq!(status, 201, "{movies_lib}");
    let movies_id = movies_lib["id"].as_str().unwrap().to_string();
    assert_eq!(movies_lib["paths"].as_array().unwrap().len(), 1);
    let (status, shows_lib) = client
        .req(
            Method::POST,
            "/api/v1/admin/libraries",
            Some(&admin),
            Some(json!({"name": "Shows", "kind": "shows", "paths": [shows.to_string_lossy()] })),
        )
        .await;
    assert_eq!(status, 201, "{shows_lib}");
    let shows_id = shows_lib["id"].as_str().unwrap().to_string();
    for lib in [&movies_id, &shows_id] {
        let (status, _) = client
            .req(
                Method::POST,
                &format!("/api/v1/admin/libraries/{lib}/access"),
                Some(&admin),
                Some(json!({"profile_id": profile_id})),
            )
            .await;
        assert_eq!(status, 204);
    }

    // C6: scan both libraries.
    let job = scan_and_wait(&client, &admin, &movies_id).await;
    assert_eq!(job["progress_total"], 1, "sample skipped: {job}");
    scan_and_wait(&client, &admin, &shows_id).await;

    // Movies: one movie item, parsed title/year, probed streams,
    // embedded + sidecar subtitles.
    let (status, items) = client
        .req(
            Method::GET,
            &format!("/api/v1/libraries/{movies_id}/items"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, 200, "{items}");
    let items = items.as_array().unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["title"], "Test Movie");
    assert_eq!(items[0]["year"], 2021);
    assert_eq!(items[0]["kind"], "movie");
    let movie_id = items[0]["id"].as_str().unwrap().to_string();

    let (status, detail) = client
        .req(
            Method::GET,
            &format!("/api/v1/items/{movie_id}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, 200, "{detail}");
    let files = detail["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["container"], "matroska");
    assert!(files[0]["duration_ms"].as_i64().unwrap() > 1000);
    let streams = files[0]["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 2, "{streams:?}");
    assert!(streams
        .iter()
        .any(|s| s["kind"] == "video" && s["codec"] == "h264"));
    let subs = files[0]["subtitles"].as_array().unwrap();
    assert_eq!(subs.len(), 2, "{subs:?}");
    assert!(subs.iter().any(|s| s["source"] == "embedded"));
    assert!(subs
        .iter()
        .any(|s| s["source"] == "sidecar" && s["language"] == "en"));

    // Shows: series -> season -> episode (C5).
    let (status, items) = client
        .req(
            Method::GET,
            &format!("/api/v1/libraries/{shows_id}/items"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let items = items.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["kind"], "series");
    assert_eq!(items[0]["title"], "Test Show");
    let series_id = items[0]["id"].as_str().unwrap().to_string();
    let (status, detail) = client
        .req(
            Method::GET,
            &format!("/api/v1/items/{series_id}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let seasons = detail["children"].as_array().unwrap();
    assert_eq!(seasons.len(), 1);
    assert_eq!(seasons[0]["season_number"], 1);
    let season_id = seasons[0]["id"].as_str().unwrap().to_string();
    let (status, detail) = client
        .req(
            Method::GET,
            &format!("/api/v1/items/{season_id}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let episodes = detail["children"].as_array().unwrap();
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0]["episode_number"], 1);
    assert_eq!(episodes[0]["title"], "Pilot");

    // Idempotent re-scan (C6): same items, no duplicates.
    scan_and_wait(&client, &admin, &movies_id).await;
    let (status, items) = client
        .req(
            Method::GET,
            &format!("/api/v1/libraries/{movies_id}/items"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(items.as_array().unwrap().len(), 1);

    // C8: delete the file; the item/file rows survive, unavailable.
    std::fs::remove_file(&movie_file).unwrap();
    scan_and_wait(&client, &admin, &movies_id).await;
    let (status, detail) = client
        .req(
            Method::GET,
            &format!("/api/v1/items/{movie_id}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, 200, "{detail}");
    assert_eq!(detail["available"], false);
    assert_eq!(detail["files"][0]["available"], false);

    std::fs::remove_dir_all(&root).ok();
}
