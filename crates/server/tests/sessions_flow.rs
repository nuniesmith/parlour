//! End-to-end test of the Now Playing sessions payload and the
//! admin stop control (UI3): the plan playback-info handed out
//! shows up on the session with per-stream modes, client and
//! remote details are recorded, and a stop revokes the stream
//! token and answers the next progress report with 409.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL and
//! ffmpeg/ffprobe on PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test sessions_flow -- --ignored

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

use reqwest::header::{COOKIE, RANGE, SET_COOKIE};
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn ffmpeg(args: &[&str]) {
    let status = Command::new("ffmpeg")
        .args(args)
        .status()
        .expect("ffmpeg available");
    assert!(status.success(), "ffmpeg failed: {args:?}");
}

fn make_mp4(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    ffmpeg(&[
        "-y",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=teal:s=320x240:d=4",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=4",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-shortest",
        path.to_str().unwrap(),
    ]);
}

fn make_mkv_with_subs(dir: &Path, name: &str, srt: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    ffmpeg(&[
        "-y",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=maroon:s=320x240:d=4",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=330:duration=4",
        "-i",
        srt.to_str().unwrap(),
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-map",
        "2:s",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-c:s",
        "srt",
        "-shortest",
        path.to_str().unwrap(),
    ]);
}

struct TestApp {
    base: String,
    client: reqwest::Client,
    cookie: String,
}

impl TestApp {
    async fn req(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
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

    /// Raw GET (no cookie) for token-authed media URLs.
    async fn raw(&self, path: &str, range: Option<&str>) -> (u16, Vec<u8>, String) {
        let mut builder = self.client.get(format!("{}{}", self.base, path));
        if let Some(r) = range {
            builder = builder.header(RANGE, r);
        }
        let resp = builder.send().await.expect("raw request");
        let status = resp.status().as_u16();
        let content_range = resp
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let bytes = resp.bytes().await.unwrap_or_default().to_vec();
        (status, bytes, content_range)
    }
}

async fn wait_for_job(app: &TestApp, job_id: &str) {
    for _ in 0..60 {
        let (_, job) = app
            .req(
                reqwest::Method::GET,
                &format!("/api/v1/admin/jobs/{job_id}"),
                None,
            )
            .await;
        match job["status"].as_str() {
            Some("succeeded") => return,
            Some("failed") => panic!("job failed: {job}"),
            _ => sleep(Duration::from_millis(500)).await,
        }
    }
    panic!("job did not finish");
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL and ffmpeg"]
async fn sessions_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test database url");
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let pool = parlour_db::connect(&db_url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");

    let fixtures = PathBuf::from(format!("/tmp/parlour-sessions-fixtures-{port}"));
    let _ = std::fs::remove_dir_all(&fixtures);
    make_mp4(
        &fixtures.join("Direct Movie (2020)"),
        "Direct Movie (2020).mp4",
    );
    let srt = fixtures.join("subs.srt");
    std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:02,500\nHello subtitles\n").unwrap();
    let season = fixtures.join("Test Show").join("Season 1");
    make_mkv_with_subs(&season, "Test Show S01E01.mkv", &srt);
    make_mkv_with_subs(&season, "Test Show S01E02.mkv", &srt);

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

    let cache_dir = fixtures.join("cache");
    let mut state = parlour_server::http::AppState::new(pool.clone(), None, false);
    std::sync::Arc::get_mut(&mut state.playback)
        .expect("fresh playback state")
        .configure(cache_dir, None);
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

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"username": "jordan", "password": "admin-pass-123"}))
        .send()
        .await
        .expect("login");
    assert_eq!(resp.status().as_u16(), 200);
    let cookie = resp
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let app = TestApp {
        base: base.clone(),
        client,
        cookie,
    };

    let (_, profiles) = app
        .req(reqwest::Method::GET, "/api/v1/profiles", None)
        .await;
    let profile_id = profiles[0]["id"].as_str().unwrap().to_string();
    let (status, _) = app
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/profiles/{profile_id}/select"),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 200);

    let (status, lib) = app
        .req(
            reqwest::Method::POST,
            "/api/v1/admin/libraries",
            Some(json!({"name": "Media", "kind": "movies", "paths": [fixtures.to_str().unwrap()]})),
        )
        .await;
    assert_eq!(status, 201, "{lib}");
    let library_id = lib["id"].as_str().unwrap().to_string();
    let (status, _) = app
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/admin/libraries/{library_id}/access"),
            Some(json!({"profile_id": profile_id})),
        )
        .await;
    assert_eq!(status, 204);
    let (status, job) = app
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/admin/libraries/{library_id}/scan"),
            None,
        )
        .await;
    assert_eq!(status, 202);
    parlour_server::catalog::spawn_background(
        pool.clone(),
        parlour_server::metadata::MetadataSettings::default(),
        parlour_catalog::EmbySettings::default(),
    );
    wait_for_job(&app, job["id"].as_str().unwrap()).await;

    let (_, items) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/libraries/{library_id}/items"),
            None,
        )
        .await;
    let movie_id = items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"] == "Direct Movie")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let series_id = items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"] == "Test Show")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, series_detail) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/items/{series_id}"),
            None,
        )
        .await;
    let season_id = series_detail["children"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, season_detail) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/items/{season_id}"),
            None,
        )
        .await;
    let episodes = season_detail["children"].as_array().unwrap();
    assert_eq!(episodes.len(), 2);
    let ep1_id = episodes.iter().find(|e| e["episode_number"] == 1).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let info_path = |id: &str| format!("/api/v1/items/{id}/playback-info?client=web&network=lan");

    // Nothing is playing yet.
    let (status, sessions) = app
        .req(reqwest::Method::GET, "/api/v1/admin/sessions", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(sessions.as_array().unwrap().len(), 0);

    // --- Episode (MKV -> remux): the session carries the plan it was given.
    let (status, ep_info) = app
        .req(reqwest::Method::GET, &info_path(&ep1_id), None)
        .await;
    assert_eq!(status, 200, "{ep_info}");
    assert_eq!(ep_info["action"], "Remux");
    let ep_file = ep_info["file_id"].as_str().unwrap().to_string();

    let resp = app
        .client
        .post(format!("{base}/api/v1/playback/start"))
        .header(COOKIE, &app.cookie)
        .header("user-agent", "ParlourTest/1.0")
        .header("x-forwarded-for", "203.0.113.7, 10.0.0.1")
        .json(&json!({
            "item_id": ep1_id,
            "file_id": ep_file,
            "position_ms": 0,
            "duration_ms": 4000,
            "client_name": "  Parlour Web  ",
            "client_version": "0.1.0",
            "device_name": "Test Browser",
        }))
        .send()
        .await
        .expect("start");
    assert_eq!(resp.status().as_u16(), 200);
    let started: Value = resp.json().await.unwrap();
    let ep_session = started["session_id"].as_str().unwrap().to_string();

    let (_, sessions) = app
        .req(reqwest::Method::GET, "/api/v1/admin/sessions", None)
        .await;
    assert_eq!(sessions.as_array().unwrap().len(), 1, "{sessions}");
    let s = &sessions[0];
    assert_eq!(s["session_id"], ep_session.as_str());
    assert_eq!(s["username"], "jordan");
    assert_eq!(s["item_kind"], "episode");
    assert_eq!(s["series_title"], "Test Show");
    assert_eq!(s["season_number"], 1);
    assert_eq!(s["episode_number"], 1);
    assert!(s["poster_item_id"].is_null(), "no artwork in this fixture");
    assert_eq!(s["duration_ms"], 4000);
    assert_eq!(s["plan"]["action"], "Remux", "{s}");
    assert_eq!(s["plan"]["reason"], "ContainerUnsupported");
    assert_eq!(s["plan"]["delivery"], "hls");
    assert_eq!(s["plan"]["client"], "web");
    assert_eq!(s["plan"]["network"], "lan");
    assert_eq!(s["plan"]["video"]["codec"], "h264");
    assert_eq!(s["plan"]["video"]["width"], 320);
    assert_eq!(s["plan"]["video"]["height"], 240);
    assert_eq!(s["plan"]["video"]["mode"], "copy");
    assert_eq!(s["plan"]["audio"]["codec"], "aac");
    assert_eq!(s["plan"]["audio"]["mode"], "copy");
    assert_eq!(s["client"]["name"], "Parlour Web", "labels are trimmed");
    assert_eq!(s["client"]["version"], "0.1.0");
    assert_eq!(s["client"]["device"], "Test Browser");
    assert_eq!(s["client"]["user_agent"], "ParlourTest/1.0");
    assert_eq!(s["remote"]["ip"], "127.0.0.1");
    assert_eq!(
        s["remote"]["forwarded_for"], "203.0.113.7",
        "first hop only"
    );
    assert_eq!(s["remote"]["protocol"], "HTTP/1.1");

    // --- Movie (MP4 -> direct play), second concurrent session.
    let (status, movie_info) = app
        .req(reqwest::Method::GET, &info_path(&movie_id), None)
        .await;
    assert_eq!(status, 200, "{movie_info}");
    assert_eq!(movie_info["action"], "DirectPlay");
    let movie_file = movie_info["file_id"].as_str().unwrap().to_string();
    let movie_url = movie_info["stream_url"].as_str().unwrap().to_string();
    let (status, _, _) = app.raw(&movie_url, Some("bytes=0-99")).await;
    assert_eq!(status, 206, "token stream works before the stop");

    let (status, started) = app
        .req(
            reqwest::Method::POST,
            "/api/v1/playback/start",
            Some(json!({"item_id": movie_id, "file_id": movie_file, "position_ms": 0})),
        )
        .await;
    assert_eq!(status, 200, "{started}");
    let movie_session = started["session_id"].as_str().unwrap().to_string();

    let (_, sessions) = app
        .req(reqwest::Method::GET, "/api/v1/admin/sessions", None)
        .await;
    assert_eq!(sessions.as_array().unwrap().len(), 2, "{sessions}");
    let m = sessions
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["session_id"] == movie_session.as_str())
        .expect("movie session listed");
    assert_eq!(m["item_kind"], "movie");
    assert!(m["series_title"].is_null());
    assert_eq!(m["plan"]["action"], "DirectPlay", "{m}");
    assert_eq!(m["plan"]["reason"], "Direct");
    assert_eq!(m["plan"]["delivery"], "file");
    assert_eq!(m["plan"]["video"]["mode"], "direct_play");
    assert_eq!(m["plan"]["audio"]["mode"], "direct_play");
    assert!(m["client"]["name"].is_null());
    assert!(m["remote"]["forwarded_for"].is_null());
    assert!(
        m["duration_ms"].as_i64().unwrap_or(0) > 0,
        "falls back to the probed file duration: {m}"
    );

    // --- Admin stop.
    let anon = reqwest::Client::new()
        .post(format!("{base}/api/v1/admin/sessions/{movie_session}/stop"))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status().as_u16(), 401);
    let stop_path = format!("/api/v1/admin/sessions/{movie_session}/stop");
    let (status, body) = app.req(reqwest::Method::POST, &stop_path, None).await;
    assert_eq!(status, 204, "{body}");

    // Gone from Now Playing; the episode session is untouched.
    let (_, sessions) = app
        .req(reqwest::Method::GET, "/api/v1/admin/sessions", None)
        .await;
    assert_eq!(sessions.as_array().unwrap().len(), 1, "{sessions}");
    assert_eq!(sessions[0]["session_id"], ep_session.as_str());

    // The old stream URL is refused — with or without the cookie.
    let (status, _, _) = app.raw(&movie_url, Some("bytes=0-99")).await;
    assert_eq!(status, 401, "revoked token");
    let with_cookie = app
        .client
        .get(format!("{base}{movie_url}"))
        .header(COOKIE, &app.cookie)
        .header(RANGE, "bytes=0-99")
        .send()
        .await
        .unwrap();
    assert_eq!(
        with_cookie.status().as_u16(),
        401,
        "the session cookie must not re-authorise a stopped stream"
    );

    // The player's next heartbeat learns it was stopped; the position still saves.
    let (status, body) = app
        .req(
            reqwest::Method::POST,
            "/api/v1/playback/progress",
            Some(json!({"item_id": movie_id, "session_id": movie_session, "position_ms": 1500})),
        )
        .await;
    assert_eq!(status, 409, "{body}");
    let (_, detail) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/items/{movie_id}"),
            None,
        )
        .await;
    assert_eq!(detail["user_state"]["position_ms"], 1500);

    // Stopping twice, or an unknown session, is 404.
    let (status, _) = app.req(reqwest::Method::POST, &stop_path, None).await;
    assert_eq!(status, 404);
    let (status, _) = app
        .req(
            reqwest::Method::POST,
            "/api/v1/admin/sessions/00000000-0000-0000-0000-000000000000/stop",
            None,
        )
        .await;
    assert_eq!(status, 404);

    // Pressing play again is a new plan and a new token, which works.
    sleep(Duration::from_millis(20)).await;
    let (_, again) = app
        .req(reqwest::Method::GET, &info_path(&movie_id), None)
        .await;
    let new_url = again["stream_url"].as_str().unwrap().to_string();
    assert_ne!(new_url, movie_url);
    let (status, _, _) = app.raw(&new_url, Some("bytes=0-99")).await;
    assert_eq!(status, 206, "a fresh token is not revoked");
    let (status, _, _) = app.raw(&movie_url, Some("bytes=0-99")).await;
    assert_eq!(status, 401, "the old token stays revoked");

    // --- The untouched session keeps reporting, then ends by id.
    let (status, _) = app
        .req(
            reqwest::Method::POST,
            "/api/v1/playback/progress",
            Some(json!({
                "item_id": ep1_id, "session_id": ep_session,
                "position_ms": 2000, "duration_ms": 4000
            })),
        )
        .await;
    assert_eq!(status, 204);
    let (_, sessions) = app
        .req(reqwest::Method::GET, "/api/v1/admin/sessions", None)
        .await;
    assert_eq!(sessions[0]["position_ms"], 2000);
    let (status, _) = app
        .req(
            reqwest::Method::POST,
            "/api/v1/playback/stopped",
            Some(json!({"item_id": ep1_id, "session_id": ep_session, "position_ms": 2000})),
        )
        .await;
    assert_eq!(status, 204);
    let (_, sessions) = app
        .req(reqwest::Method::GET, "/api/v1/admin/sessions", None)
        .await;
    assert_eq!(sessions.as_array().unwrap().len(), 0);

    let _ = std::fs::remove_dir_all(&fixtures);
    server.abort();
}
