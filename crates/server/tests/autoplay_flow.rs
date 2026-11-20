//! End-to-end test of SEG6's server half (Phase 9, D23):
//! playback-info names the episode that follows in series order —
//! season, then episode, on into later seasons, watched or not,
//! skipping episodes with no available file, never leaving the
//! series — and carries the active profile's autoplay choice
//! (`autoplay_next`, default on), read fresh on every call and kept
//! per profile. Then the reporting a roll makes, as the web player
//! sends it (D23): the stopped report for the old episode — at the
//! end of the file when its credits run there, otherwise where the
//! playhead is — then the start report for the next one, at its
//! resume point (partly watched: where the viewer left off; played:
//! the beginning), so Continue Watching, Next Up, play counts and
//! the activity feed stay truthful.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a fresh
//! database of its own) and ffmpeg/ffprobe on PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test autoplay_flow -- --ignored

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

use reqwest::header::{COOKIE, SET_COOKIE};
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

/// A 4 s H.264/AAC file: direct play for the web client.
fn make_mp4(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    let status = Command::new("ffmpeg")
        .args([
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
        ])
        .status()
        .expect("ffmpeg available");
    assert!(status.success(), "ffmpeg failed for {name}");
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

    async fn get(&self, path: &str) -> (u16, Value) {
        self.req(reqwest::Method::GET, path, None).await
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        self.req(reqwest::Method::POST, path, Some(body)).await
    }

    /// playback-info for the web client on the LAN, as the player asks.
    async fn info(&self, item: Uuid) -> Value {
        let (status, info) = self
            .get(&format!(
                "/api/v1/items/{item}/playback-info?client=web&network=lan"
            ))
            .await;
        assert_eq!(status, 200, "{info}");
        info
    }

    /// The id of playback-info's `next_episode`, or None when it is null.
    async fn next_of(&self, item: Uuid) -> Option<Uuid> {
        let info = self.info(item).await;
        let next = &info["next_episode"];
        assert!(
            info.as_object().unwrap().contains_key("next_episode"),
            "next_episode is always present, null when there is none: {info}"
        );
        next["id"].as_str().map(|id| id.parse().unwrap())
    }

    async fn select(&self, profile: &str) {
        let (status, body) = self
            .post(&format!("/api/v1/profiles/{profile}/select"), json!({}))
            .await;
        assert_eq!(status, 200, "{body}");
    }

    async fn set_autoplay(&self, profile: &str, on: bool) {
        let (status, body) = self
            .req(
                reqwest::Method::PATCH,
                &format!("/api/v1/profiles/{profile}"),
                Some(json!({"autoplay_next": on})),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["autoplay_next"], on, "{body}");
    }

    async fn user_state(&self, item: Uuid) -> Value {
        let (status, detail) = self.get(&format!("/api/v1/items/{item}")).await;
        assert_eq!(status, 200, "{detail}");
        detail["user_state"].clone()
    }
}

async fn wait_for_job(app: &TestApp, job_id: &str) {
    for _ in 0..120 {
        let (_, job) = app.get(&format!("/api/v1/admin/jobs/{job_id}")).await;
        match job["status"].as_str() {
            Some("succeeded") => return,
            Some("failed") => panic!("job failed: {job}"),
            _ => sleep(Duration::from_millis(500)).await,
        }
    }
    panic!("job did not finish");
}

fn ids(rows: &Value) -> Vec<String> {
    rows.as_array()
        .unwrap_or_else(|| panic!("not a list: {rows}"))
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL and ffmpeg"]
async fn autoplay_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test database url");
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let pool = parlour_db::connect(&db_url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");

    // Roll Show: specials (season 0), two seasons; Other Show: one
    // episode numbered after all of Roll Show's; and a movie.
    let fixtures = PathBuf::from(format!("/tmp/parlour-autoplay-fixtures-{port}"));
    let _ = std::fs::remove_dir_all(&fixtures);
    let roll = fixtures.join("Roll Show");
    make_mp4(&roll.join("Specials"), "Roll Show S00E01.mp4");
    for e in 1..=4 {
        make_mp4(&roll.join("Season 1"), &format!("Roll Show S01E0{e}.mp4"));
    }
    for e in 1..=2 {
        make_mp4(&roll.join("Season 2"), &format!("Roll Show S02E0{e}.mp4"));
    }
    make_mp4(
        &fixtures.join("Other Show").join("Season 3"),
        "Other Show S03E01.mp4",
    );
    make_mp4(&fixtures.join("Some Film (2020)"), "Some Film (2020).mp4");

    let mut vars = HashMap::new();
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

    let (_, profiles) = app.get("/api/v1/profiles").await;
    let profile_id = profiles[0]["id"].as_str().unwrap().to_string();
    app.select(&profile_id).await;

    let (status, lib) = app
        .post(
            "/api/v1/admin/libraries",
            json!({"name": "Shows", "kind": "shows", "paths": [fixtures.to_str().unwrap()]}),
        )
        .await;
    assert_eq!(status, 201, "{lib}");
    let library_id = lib["id"].as_str().unwrap().to_string();
    let (status, _) = app
        .post(
            &format!("/api/v1/admin/libraries/{library_id}/access"),
            json!({"profile_id": profile_id}),
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

    // Every episode by (series, season, episode).
    let rows: Vec<(Uuid, String, i32, i32)> = sqlx::query_as(
        "SELECT i.id, sr.title, i.season_number, i.episode_number
         FROM media_items i
         JOIN media_items se ON se.id = i.parent_id
         JOIN media_items sr ON sr.id = se.parent_id
         WHERE i.kind = 'episode'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 8, "{rows:?}");
    let ep = |series: &str, season: i32, episode: i32| -> Uuid {
        rows.iter()
            .find(|(_, t, s, e)| t == series && *s == season && *e == episode)
            .unwrap_or_else(|| panic!("no {series} S{season}E{episode} in {rows:?}"))
            .0
    };
    let sp1 = ep("Roll Show", 0, 1);
    let s1e1 = ep("Roll Show", 1, 1);
    let s1e2 = ep("Roll Show", 1, 2);
    let s1e3 = ep("Roll Show", 1, 3);
    let s1e4 = ep("Roll Show", 1, 4);
    let s2e1 = ep("Roll Show", 2, 1);
    let s2e2 = ep("Roll Show", 2, 2);
    let other = ep("Other Show", 3, 1);
    let (film,): (Uuid,) = sqlx::query_as("SELECT id FROM media_items WHERE kind = 'movie'")
        .fetch_one(&pool)
        .await
        .unwrap();

    // --- D23: the next episode in series order, and what it carries.
    let info = app.info(s1e1).await;
    assert_eq!(info["autoplay_next"], true, "on by default (D15): {info}");
    let next = &info["next_episode"];
    assert_eq!(next["id"], s1e2.to_string(), "{info}");
    assert_eq!(next["kind"], "episode");
    assert_eq!(next["title"], "Episode 2");
    assert_eq!(next["season_number"], 1);
    assert_eq!(next["episode_number"], 2);
    assert_eq!(next["series_title"], "Roll Show");
    assert!(
        next["poster_item_id"].is_null(),
        "no artwork in this library, so the card shows a plain tile: {next}"
    );
    assert_eq!(next["available"], true);

    assert_eq!(app.next_of(s1e2).await, Some(s1e3));
    assert_eq!(app.next_of(s1e3).await, Some(s1e4));
    assert_eq!(
        app.next_of(s1e4).await,
        Some(s2e1),
        "on into the next season"
    );
    assert_eq!(app.next_of(s2e1).await, Some(s2e2));
    assert_eq!(
        app.next_of(s2e2).await,
        None,
        "the last episode has none — not another series' later-numbered one"
    );
    assert_eq!(
        app.next_of(sp1).await,
        Some(s1e1),
        "season 0 sorts first, so the last special rolls into S1:E1"
    );
    assert_eq!(app.next_of(other).await, None, "a series of one episode");
    let film_info = app.info(film).await;
    assert!(film_info["next_episode"].is_null(), "{film_info}");
    assert_eq!(film_info["autoplay_next"], true, "{film_info}");

    // --- Skipping episodes with no available file (playback-info
    // refuses them), read live on every call.
    let set_available = |item: Uuid, available: bool| {
        let pool = pool.clone();
        async move {
            sqlx::query("UPDATE media_files SET available = $2 WHERE item_id = $1")
                .bind(item)
                .bind(available)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    set_available(s1e3, false).await;
    set_available(s2e1, false).await;
    let (status, refused) = app
        .get(&format!(
            "/api/v1/items/{s1e3}/playback-info?client=web&network=lan"
        ))
        .await;
    assert_eq!(status, 400, "no playable file: {refused}");
    assert_eq!(app.next_of(s1e2).await, Some(s1e4), "S1:E3 is skipped");
    assert_eq!(
        app.next_of(s1e4).await,
        Some(s2e2),
        "S2:E1 is skipped, across the season boundary"
    );
    set_available(s1e3, true).await;
    set_available(s2e1, true).await;
    assert_eq!(app.next_of(s1e2).await, Some(s1e3));
    assert_eq!(app.next_of(s1e4).await, Some(s2e1));

    // --- The profile's choice: applies on the next call, sent with
    // the next episode either way (the player decides), kept per
    // profile.
    app.set_autoplay(&profile_id, false).await;
    let info = app.info(s1e1).await;
    assert_eq!(info["autoplay_next"], false, "{info}");
    assert_eq!(info["next_episode"]["id"], s1e2.to_string(), "{info}");
    let (status, kids) = app.post("/api/v1/profiles", json!({"name": "Kids"})).await;
    assert_eq!(status, 201, "{kids}");
    assert_eq!(kids["autoplay_next"], true, "{kids}");
    let kids_id = kids["id"].as_str().unwrap().to_string();
    let (status, _) = app
        .post(
            &format!("/api/v1/admin/libraries/{library_id}/access"),
            json!({"profile_id": kids_id}),
        )
        .await;
    assert_eq!(status, 204);
    app.select(&kids_id).await;
    let kids_info = app.info(s1e1).await;
    assert_eq!(kids_info["autoplay_next"], true, "{kids_info}");
    assert_eq!(kids_info["next_episode"], info["next_episode"]);
    app.set_autoplay(&kids_id, false).await;
    app.set_autoplay(&kids_id, true).await;
    app.select(&profile_id).await;
    assert_eq!(app.info(s1e1).await["autoplay_next"], false);
    app.set_autoplay(&profile_id, true).await;
    assert_eq!(app.info(s1e1).await["autoplay_next"], true);

    // --- A roll, reported the way the player does it (D23).
    // Earlier, S1:E2 was watched partway: 1 s of 4.
    let (status, s) = app
        .post(
            "/api/v1/playback/start",
            json!({"item_id": s1e2, "position_ms": 0, "duration_ms": 4000}),
        )
        .await;
    assert_eq!(status, 200, "{s}");
    let (status, _) = app
        .post(
            "/api/v1/playback/stopped",
            json!({"item_id": s1e2, "session_id": s["session_id"],
                   "position_ms": 1000, "duration_ms": 4000}),
        )
        .await;
    assert_eq!(status, 204);

    // Now S1:E1 plays, into credits that run to the end of the file.
    let (status, s1) = app
        .post(
            "/api/v1/playback/start",
            json!({"item_id": s1e1, "position_ms": 0, "duration_ms": 4000}),
        )
        .await;
    assert_eq!(status, 200, "{s1}");
    let (status, _) = app
        .post(
            "/api/v1/playback/progress",
            json!({"item_id": s1e1, "session_id": s1["session_id"],
                   "position_ms": 3000, "duration_ms": 4000}),
        )
        .await;
    assert_eq!(status, 204);
    let (_, home) = app.get("/api/v1/home").await;
    assert!(ids(&home["continue_watching"]).contains(&s1e1.to_string()));

    // The countdown fires at 3.2 s, inside those credits: the stopped
    // report goes out at the end of the file, as for D22's Skip
    // Credits, so the episode counts as played...
    let (status, _) = app
        .post(
            "/api/v1/playback/stopped",
            json!({"item_id": s1e1, "session_id": s1["session_id"],
                   "position_ms": 4000, "duration_ms": 4000}),
        )
        .await;
    assert_eq!(status, 204);
    let state = app.user_state(s1e1).await;
    assert_eq!(state["played"], true, "{state}");
    assert_eq!(state["play_count"], 1, "{state}");

    // ...then the player asks for the next episode: partly watched, so
    // it resumes where the viewer left off (no prompt — the client's
    // side), and it knows its own next one.
    let next_info = app.info(s1e2).await;
    assert_eq!(next_info["resume_position_ms"], 1000, "{next_info}");
    assert_eq!(next_info["played"], false);
    assert_eq!(next_info["next_episode"]["id"], s1e3.to_string());

    // ...and reports its start from there.
    let (status, s2) = app
        .post(
            "/api/v1/playback/start",
            json!({"item_id": s1e2, "file_id": next_info["file_id"],
                   "position_ms": 1000, "duration_ms": 4000}),
        )
        .await;
    assert_eq!(status, 200, "{s2}");
    let state = app.user_state(s1e2).await;
    assert_eq!(state["play_count"], 2, "a second play: {state}");
    assert_eq!(
        state["position_ms"], 1000,
        "the resume point survives: {state}"
    );
    assert_eq!(state["played"], false);

    // Now Playing: only the new episode, at its resume point.
    let (_, sessions) = app.get("/api/v1/admin/sessions").await;
    let sessions = sessions.as_array().unwrap().clone();
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    assert_eq!(sessions[0]["item_id"], s1e2.to_string());
    assert_eq!(sessions[0]["position_ms"], 1000);
    assert_eq!(sessions[0]["session_id"], s2["session_id"]);

    // Activity, newest first: the new start right after the old stop.
    let (_, feed) = app
        .get("/api/v1/admin/activity?category=playback&limit=2")
        .await;
    let entries = feed["entries"].as_array().unwrap();
    assert_eq!(entries[0]["kind"], "playback.start", "{feed}");
    assert_eq!(entries[0]["item_id"], s1e2.to_string());
    assert_eq!(entries[1]["kind"], "playback.stop", "{feed}");
    assert_eq!(entries[1]["item_id"], s1e1.to_string());
    assert_eq!(entries[1]["detail"]["position_ms"], 4000);

    // Home: S1:E1 has left Continue Watching, S1:E2 is in it, and Next
    // Up offers S1:E2 (the first unplayed after a played one).
    let (_, home) = app.get("/api/v1/home").await;
    let watching = ids(&home["continue_watching"]);
    assert!(!watching.contains(&s1e1.to_string()), "{home}");
    assert!(watching.contains(&s1e2.to_string()), "{home}");
    assert!(ids(&home["next_up"]).contains(&s1e2.to_string()), "{home}");

    // A second roll, from credits that end before the file does (a
    // scene after them): the stopped report carries the real position,
    // 3 s of 4 — under the 90 % rule, so S1:E2 is not played and stays
    // in Continue Watching where it was left.
    let (status, _) = app
        .post(
            "/api/v1/playback/stopped",
            json!({"item_id": s1e2, "session_id": s2["session_id"],
                   "position_ms": 3000, "duration_ms": 4000}),
        )
        .await;
    assert_eq!(status, 204);
    let state = app.user_state(s1e2).await;
    assert_eq!(state["played"], false, "{state}");
    assert_eq!(state["position_ms"], 3000, "{state}");
    let (_, home) = app.get("/api/v1/home").await;
    assert!(ids(&home["continue_watching"]).contains(&s1e2.to_string()));

    // --- Played or not (D23): a played next episode is still the next
    // one — a rewatch rolls — and starts from the beginning, even with
    // a position saved from before it was marked played.
    let (status, s4) = app
        .post(
            "/api/v1/playback/start",
            json!({"item_id": s1e4, "position_ms": 0, "duration_ms": 4000}),
        )
        .await;
    assert_eq!(status, 200, "{s4}");
    let (status, _) = app
        .post(
            "/api/v1/playback/stopped",
            json!({"item_id": s1e4, "session_id": s4["session_id"],
                   "position_ms": 2000, "duration_ms": 4000}),
        )
        .await;
    assert_eq!(status, 204);
    assert_eq!(app.info(s1e4).await["resume_position_ms"], 2000);
    let (status, marked) = app
        .post(
            &format!("/api/v1/items/{s1e4}/mark"),
            json!({"played": true}),
        )
        .await;
    assert_eq!(status, 200, "{marked}");
    assert_eq!(marked["position_ms"], 2000, "marking keeps the position");
    assert_eq!(app.next_of(s1e3).await, Some(s1e4), "played, still next");
    let info = app.info(s1e4).await;
    assert_eq!(info["played"], true, "{info}");
    assert_eq!(info["resume_position_ms"], 0, "from the beginning: {info}");
    // S1:E1 was played by the roll above, and it still leads to S1:E2.
    assert_eq!(app.next_of(sp1).await, Some(s1e1));
    assert_eq!(app.next_of(s1e1).await, Some(s1e2));

    let _ = std::fs::remove_dir_all(&fixtures);
    server.abort();
}
