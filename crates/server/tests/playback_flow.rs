//! End-to-end test of Milestone 1.6 playback: playback-info
//! plans (direct play vs remux), signed stream tokens + HTTP
//! Range, HLS remux output, subtitle extraction to WebVTT,
//! playback reporting into user_data, Next Up, marks, and the
//! admin session registry.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL and
//! ffmpeg/ffprobe on PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test playback_flow -- --ignored

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
async fn playback_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test database url");
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let pool = parlour_db::connect(&db_url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");

    let fixtures = PathBuf::from(format!("/tmp/parlour-playback-fixtures-{port}"));
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
    let ep2_title = episodes.iter().find(|e| e["episode_number"] == 2).unwrap()["title"]
        .as_str()
        .unwrap()
        .to_string();

    // --- P1/P2: direct play plan + token stream + range
    let (status, info) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/items/{movie_id}/playback-info?client=web&network=lan"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{info}");
    assert_eq!(info["action"], "DirectPlay");
    assert_eq!(info["stream_kind"], "file");
    let stream_url = info["stream_url"].as_str().unwrap().to_string();

    let (status, bytes, _) = app.raw(&stream_url, None).await;
    assert_eq!(status, 200);
    assert!(bytes.len() > 10_000, "streamed {} bytes", bytes.len());
    let (status, part, content_range) = app.raw(&stream_url, Some("bytes=100-199")).await;
    assert_eq!(status, 206);
    assert_eq!(part.len(), 100);
    assert!(
        content_range.starts_with("bytes 100-199/"),
        "{content_range}"
    );
    let (status, _, _) = app
        .raw(&stream_url.replace("token=", "token=x"), None)
        .await;
    assert_eq!(status, 401, "bad token must not stream");

    // --- P1/P3/P6: episode remux plan, HLS output, subtitles
    let (_, ep_info) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/items/{ep1_id}/playback-info?client=web&network=lan"),
            None,
        )
        .await;
    assert_eq!(ep_info["action"], "Remux", "{ep_info}");
    assert_eq!(ep_info["reason"], "ContainerUnsupported");
    assert_eq!(ep_info["stream_kind"], "hls");
    let subs = ep_info["subtitles"].as_array().unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0]["text"], true);
    let sub_url = subs[0]["url"].as_str().unwrap().to_string();
    let (status, vtt, _) = app.raw(&sub_url, None).await;
    assert_eq!(status, 200);
    assert!(
        String::from_utf8_lossy(&vtt).contains("Hello subtitles"),
        "vtt: {}",
        String::from_utf8_lossy(&vtt)
    );

    let playlist_url = ep_info["stream_url"].as_str().unwrap().to_string();
    let mut playlist = Vec::new();
    for _ in 0..30 {
        let (status, body, _) = app.raw(&playlist_url, None).await;
        if status == 200 {
            playlist = body;
            break;
        }
        sleep(Duration::from_secs(1)).await;
    }
    let playlist_text = String::from_utf8_lossy(&playlist).to_string();
    assert!(
        playlist_text.contains("#EXTM3U"),
        "playlist: {playlist_text}"
    );
    assert!(
        playlist_text.contains("init.mp4"),
        "playlist: {playlist_text}"
    );
    let seg_url = playlist_url.replace("playlist.m3u8", "seg-00000.m4s");
    let (status, seg, _) = app.raw(&seg_url, None).await;
    assert_eq!(status, 200);
    assert!(!seg.is_empty());

    // --- P4/P8: reporting + session registry
    let (status, session) = app
        .req(
            reqwest::Method::POST,
            "/api/v1/playback/start",
            Some(json!({"item_id": ep1_id, "position_ms": 0, "duration_ms": 4000})),
        )
        .await;
    assert_eq!(status, 200, "{session}");
    let (status, sessions) = app
        .req(reqwest::Method::GET, "/api/v1/admin/sessions", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(sessions.as_array().unwrap().len(), 1, "{sessions}");
    assert_eq!(sessions[0]["item_id"], ep1_id);

    let (status, _) = app
        .req(
            reqwest::Method::POST,
            "/api/v1/playback/progress",
            Some(json!({"item_id": ep1_id, "position_ms": 1000, "duration_ms": 4000})),
        )
        .await;
    assert_eq!(status, 204);
    let (_, detail) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/items/{ep1_id}"),
            None,
        )
        .await;
    assert_eq!(detail["user_state"]["position_ms"], 1000);
    assert_eq!(detail["user_state"]["played"], false);
    assert_eq!(detail["user_state"]["play_count"], 1);

    // 95% → played at the ≥90% rule.
    let (status, _) = app
        .req(
            reqwest::Method::POST,
            "/api/v1/playback/progress",
            Some(json!({"item_id": ep1_id, "position_ms": 3800, "duration_ms": 4000})),
        )
        .await;
    assert_eq!(status, 204);
    let (_, detail) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/items/{ep1_id}"),
            None,
        )
        .await;
    assert_eq!(detail["user_state"]["played"], true);

    // --- P7: Next Up surfaces episode 2
    let (_, home) = app.req(reqwest::Method::GET, "/api/v1/home", None).await;
    let next_up_titles: Vec<&str> = home["next_up"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap())
        .collect();
    assert!(
        next_up_titles.contains(&ep2_title.as_str()),
        "{next_up_titles:?}"
    );

    // --- P5: marks
    let (status, state) = app
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/items/{ep1_id}/mark"),
            Some(json!({"favorite": true})),
        )
        .await;
    assert_eq!(status, 200, "{state}");
    assert_eq!(state["is_favorite"], true);
    let (status, state) = app
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/items/{ep1_id}/mark"),
            Some(json!({"played": false})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(state["played"], false);
    assert_eq!(state["position_ms"], 0);
    assert_eq!(
        state["is_favorite"], true,
        "marks must not clobber each other"
    );

    // Stopped clears the live session.
    let (status, _) = app
        .req(
            reqwest::Method::POST,
            "/api/v1/playback/stopped",
            Some(json!({"item_id": ep1_id, "position_ms": 0, "duration_ms": 4000})),
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
