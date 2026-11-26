//! End-to-end test of AB8 (D29/D35): listening sessions opened, kept
//! and closed through the playback reports the web player sends, with
//! no `user_data` write (no play count); the open session in Now
//! Playing and in the profile's own list; the closed one in the
//! Activity feed; a new start replacing an open one; an admin stop; a
//! session closed after an hour's silence and a fresh one opened by its
//! next report; per-profile listening stats; and the Home shelves —
//! Continue Listening (in progress, not finished, not hidden; the hide
//! clearing on the next move), Continue Series, Recently Added, Recent
//! Series, Discover, Listen Again and Newest Authors — plus the Home
//! Screen rail that can't be hidden.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a fresh
//! database) and ffmpeg/ffprobe on PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test listening_flow -- --ignored

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

use reqwest::header::{COOKIE, SET_COOKIE};
use reqwest::Method;
use serde_json::{json, Value};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// An untagged 30 s MP3, so the folder names speak and a position of
/// a few seconds is well short of the 10 s auto-finish point.
fn make_mp3(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let out = dir.join("01.mp3");
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=30",
            "-c:a",
            "libmp3lame",
            "-b:a",
            "32k",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("ffmpeg available");
    assert!(status.success(), "ffmpeg failed for {}", out.display());
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

    async fn ok(&self, path: &str) -> Value {
        let (status, v) = self.req(Method::GET, path, None).await;
        assert_eq!(status, 200, "{path}: {v}");
        v
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        self.req(Method::POST, path, Some(body)).await
    }

    async fn put(&self, path: &str, body: Value) -> Value {
        let (status, v) = self.req(Method::PUT, path, Some(body.clone())).await;
        assert_eq!(status, 200, "PUT {path} {body}: {v}");
        v
    }

    async fn select(&self, profile_id: &str) {
        let (status, v) = self
            .post(&format!("/api/v1/profiles/{profile_id}/select"), json!({}))
            .await;
        assert_eq!(status, 200, "{v}");
    }
}

fn titles(books: &Value) -> Vec<String> {
    books
        .as_array()
        .unwrap_or_else(|| panic!("not a list: {books}"))
        .iter()
        .map(|b| b["title"].as_str().unwrap().to_string())
        .collect()
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn names(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap().to_string())
        .collect()
}

async fn activity(app: &TestApp, kind: &str) -> Vec<Value> {
    let page = app.ok("/api/v1/admin/activity?limit=200").await;
    page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == kind)
        .cloned()
        .collect()
}

#[derive(sqlx::FromRow, Debug)]
struct Row {
    ended_at: Option<chrono::DateTime<chrono::Utc>>,
    last_seen_at: chrono::DateTime<chrono::Utc>,
    end_reason: Option<String>,
    start_position_ms: i64,
    position_ms: i64,
    listened_ms: i64,
}

async fn row(pool: &parlour_db::PgPool, id: &str) -> Row {
    sqlx::query_as::<_, Row>(
        "SELECT ended_at, last_seen_at, end_reason, start_position_ms, position_ms, listened_ms
         FROM listening_sessions WHERE id = $1",
    )
    .bind(uuid::Uuid::parse_str(id).unwrap())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL and ffmpeg"]
async fn listening_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test database url");
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let pool = parlour_db::connect(&db_url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");

    // Library (no tags; every value is the folder's):
    //   Jane Doe/Solo Book/                 standalone
    //   Jane Doe/Saga/1. Saga One/ … 3.     a series of three
    //   Ann Writer/Another/                 standalone
    //   Ann Writer/Trilogy/1. Tri One/, 2.  a series nobody has started
    let root = PathBuf::from(format!("/tmp/parlour-ab8-fixtures-{port}"));
    let _ = std::fs::remove_dir_all(&root);
    let jane = root.join("Jane Doe");
    let ann = root.join("Ann Writer");
    make_mp3(&jane.join("Solo Book"));
    make_mp3(&jane.join("Saga").join("1. Saga One"));
    make_mp3(&jane.join("Saga").join("2. Saga Two"));
    make_mp3(&jane.join("Saga").join("3. Saga Three"));
    make_mp3(&ann.join("Another"));
    make_mp3(&ann.join("Trilogy").join("1. Tri One"));
    make_mp3(&ann.join("Trilogy").join("2. Tri Two"));

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
    let state = parlour_server::http::AppState::new(pool.clone(), None, false);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    tokio::spawn(async move {
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
    let profiles = app.ok("/api/v1/profiles").await;
    let jordan_id = profiles[0]["id"].as_str().unwrap().to_string();
    app.select(&jordan_id).await;

    let (status, lib) = app
        .post(
            "/api/v1/admin/libraries",
            json!({"name": "Audiobooks", "kind": "audiobooks",
                   "paths": [root.to_str().unwrap()]}),
        )
        .await;
    assert_eq!(status, 201, "{lib}");
    let library_id = lib["id"].as_str().unwrap().to_string();
    let (status, _) = app
        .post(
            &format!("/api/v1/admin/libraries/{library_id}/access"),
            json!({"profile_id": jordan_id}),
        )
        .await;
    assert_eq!(status, 204);
    let report =
        parlour_catalog::scan_library(&pool, uuid::Uuid::parse_str(&library_id).unwrap(), None)
            .await
            .expect("scan");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.seen_files, 7);

    let books = app.ok("/api/v1/audiobooks").await;
    let id_of = |title: &str| -> String {
        books
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["title"] == title)
            .unwrap_or_else(|| panic!("no book {title} in {books}"))["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let solo = id_of("Solo Book");
    let saga_one = id_of("Saga One");
    let saga_two = id_of("Saga Two");
    let another = id_of("Another");
    assert_eq!(books.as_array().unwrap().len(), 7, "{:?}", titles(&books));

    // ------------------------------------------- a session, start to stop
    let (status, started) = app
        .post(
            "/api/v1/playback/start",
            json!({"item_id": solo, "position_ms": 0, "duration_ms": 30000,
                   "client_name": "Parlour Web", "device_name": "Chrome on Linux"}),
        )
        .await;
    assert_eq!(status, 200, "{started}");
    let s1 = started["session_id"].as_str().unwrap().to_string();

    // No play count: a book's reports never touch user_data.
    let (user_rows,): (i64,) = sqlx::query_as("SELECT count(*) FROM user_data")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(user_rows, 0, "a listen writes no user_data row");
    assert!(
        activity(&app, "playback.start").await.is_empty(),
        "a listen opening is not an Activity line"
    );

    // Now Playing (admins) has the open session, as a listen.
    let now_playing = app.ok("/api/v1/admin/sessions").await;
    let card = now_playing
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == s1.as_str())
        .unwrap_or_else(|| panic!("no listening session in {now_playing}"));
    assert_eq!(card["item_kind"], "audiobook");
    assert_eq!(card["item_title"], "Solo Book");
    assert_eq!(card["plan"], Value::Null);
    assert_eq!(card["duration_ms"], 30000);
    assert_eq!(card["client"]["name"], "Parlour Web");

    // ...and so does the profile's own list, live.
    let mine = app.ok("/api/v1/listening/sessions").await;
    assert_eq!(mine["sessions"].as_array().unwrap().len(), 1);
    let open = &mine["sessions"][0];
    assert_eq!(open["id"], s1.as_str());
    assert_eq!(open["live"], true);
    assert_eq!(open["ended_at"], Value::Null);
    assert_eq!(open["title"], "Solo Book");
    assert_eq!(open["authors"], json!(["Jane Doe"]));

    // Progress: the player's listening time, capped by the clock.
    let (status, beat) = app
        .post(
            "/api/v1/playback/progress",
            json!({"item_id": solo, "session_id": s1, "position_ms": 5000,
                   "duration_ms": 30000, "listened_ms": 4000}),
        )
        .await;
    assert_eq!(status, 200, "{beat}");
    assert_eq!(beat["session_id"], s1.as_str());
    assert_eq!(row(&pool, &s1).await.listened_ms, 4000);
    let (status, _) = app
        .post(
            "/api/v1/playback/progress",
            json!({"item_id": solo, "session_id": s1, "position_ms": 6000,
                   "listened_ms": 600000}),
        )
        .await;
    assert_eq!(status, 200);
    let capped = row(&pool, &s1).await.listened_ms;
    assert!(
        (9000..=11_000).contains(&capped),
        "ten minutes claimed in a moment is capped at the clock + 5 s: {capped}"
    );
    let (user_rows,): (i64,) = sqlx::query_as("SELECT count(*) FROM user_data")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(user_rows, 0);

    // Stop: closed where the player left it, into the Activity feed.
    let (status, _) = app
        .post(
            "/api/v1/playback/stopped",
            json!({"item_id": solo, "session_id": s1, "position_ms": 7000,
                   "listened_ms": 1000}),
        )
        .await;
    assert_eq!(status, 204);
    let closed = row(&pool, &s1).await;
    assert!(closed.ended_at.is_some());
    assert_eq!(closed.end_reason.as_deref(), Some("stopped"));
    assert_eq!(closed.start_position_ms, 0);
    assert_eq!(closed.position_ms, 7000);
    assert_eq!(closed.listened_ms, capped + 1000);
    let now_playing = app.ok("/api/v1/admin/sessions").await;
    assert!(
        now_playing.as_array().unwrap().is_empty(),
        "a closed session leaves Now Playing: {now_playing}"
    );
    let listened = activity(&app, "playback.listened").await;
    assert_eq!(listened.len(), 1, "{listened:?}");
    let line = &listened[0];
    assert_eq!(line["actor"], "jordan");
    assert_eq!(line["item_id"], solo.as_str());
    assert_eq!(line["profile_id"], jordan_id.as_str());
    assert_eq!(
        line["summary"],
        format!(
            "jordan listened to Solo Book for {} s",
            (capped + 1000) / 1000
        )
    );
    assert_eq!(line["detail"]["session_id"], s1.as_str());
    assert_eq!(line["detail"]["end_reason"], "stopped");
    assert_eq!(line["detail"]["position_ms"], 7000);
    assert_eq!(line["detail"]["listened_ms"], capped + 1000);
    // A second stopped report (page teardown after the end) adds nothing.
    let (status, _) = app
        .post(
            "/api/v1/playback/stopped",
            json!({"item_id": solo, "session_id": s1, "position_ms": 7000}),
        )
        .await;
    assert_eq!(status, 204);
    assert_eq!(activity(&app, "playback.listened").await.len(), 1);
    let mine = app.ok("/api/v1/listening/sessions").await;
    assert_eq!(mine["sessions"][0]["live"], false);
    assert_eq!(mine["sessions"][0]["end_reason"], "stopped");

    // ---------------------------------------- a new start replaces one
    let (_, a) = app
        .post(
            "/api/v1/playback/start",
            json!({"item_id": another, "position_ms": 0}),
        )
        .await;
    let a1 = a["session_id"].as_str().unwrap().to_string();
    let (_, b) = app
        .post(
            "/api/v1/playback/start",
            json!({"item_id": another, "position_ms": 0}),
        )
        .await;
    let a2 = b["session_id"].as_str().unwrap().to_string();
    assert_eq!(
        row(&pool, &a1).await.end_reason.as_deref(),
        Some("replaced")
    );
    assert!(row(&pool, &a2).await.ended_at.is_none());
    let now_playing = app.ok("/api/v1/admin/sessions").await;
    let ids: Vec<&str> = now_playing
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [a2.as_str()], "only the new one is playing");

    // ------------------------------------------------- an admin stop
    let detail = app.ok(&format!("/api/v1/audiobooks/{another}")).await;
    let stream_url = detail["tracks"][0]["stream_url"]
        .as_str()
        .unwrap()
        .to_string();
    let plain = reqwest::Client::new();
    let streamed = plain
        .get(format!("{base}{stream_url}"))
        .send()
        .await
        .unwrap();
    assert_eq!(streamed.status().as_u16(), 200, "the track plays before");
    let (status, _) = app
        .post(&format!("/api/v1/admin/sessions/{a2}/stop"), json!({}))
        .await;
    assert_eq!(status, 204);
    assert_eq!(row(&pool, &a2).await.end_reason.as_deref(), Some("admin"));
    let (status, _) = app
        .post(
            "/api/v1/playback/progress",
            json!({"item_id": another, "session_id": a2, "position_ms": 1000}),
        )
        .await;
    assert_eq!(status, 409, "the stopped player is told why");
    assert_eq!(activity(&app, "playback.stopped_by_admin").await.len(), 1);
    // Every track's stream token was revoked with the session.
    let refused = plain
        .get(format!("{base}{stream_url}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        refused.status().as_u16(),
        401,
        "the stopped stream stays stopped"
    );
    let (status, _) = app
        .post(
            "/api/v1/playback/stopped",
            json!({"item_id": another, "session_id": a2, "position_ms": 1000}),
        )
        .await;
    assert_eq!(status, 204);
    // replaced + admin = two more listened lines (the stop added none).
    assert_eq!(activity(&app, "playback.listened").await.len(), 3);

    // ---------------------------- an hour's silence, then a report again
    let (_, c) = app
        .post(
            "/api/v1/playback/start",
            json!({"item_id": solo, "position_ms": 7000}),
        )
        .await;
    let c1 = c["session_id"].as_str().unwrap().to_string();
    sqlx::query(
        "UPDATE listening_sessions SET last_seen_at = now() - interval '2 hours' WHERE id = $1",
    )
    .bind(uuid::Uuid::parse_str(&c1).unwrap())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(parlour_server::listening::close_stale(&pool).await, 1);
    let stale = row(&pool, &c1).await;
    assert_eq!(stale.end_reason.as_deref(), Some("timeout"));
    assert_eq!(
        stale.ended_at,
        Some(stale.last_seen_at),
        "closed at its last report"
    );
    assert_eq!(parlour_server::listening::close_stale(&pool).await, 0);
    let (status, reopened) = app
        .post(
            "/api/v1/playback/progress",
            json!({"item_id": solo, "session_id": c1, "position_ms": 8000,
                   "listened_ms": 1000}),
        )
        .await;
    assert_eq!(status, 200, "{reopened}");
    let c2 = reopened["session_id"].as_str().unwrap().to_string();
    assert_ne!(c2, c1, "a fresh session for the player to carry on with");
    let fresh = row(&pool, &c2).await;
    assert!(fresh.ended_at.is_none());
    assert_eq!(fresh.start_position_ms, 8000);
    let ids: Vec<String> = app
        .ok("/api/v1/admin/sessions")
        .await
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        ids,
        std::slice::from_ref(&c2),
        "the stale one left Now Playing"
    );
    let (status, _) = app
        .post(
            "/api/v1/playback/stopped",
            json!({"item_id": solo, "session_id": c2, "position_ms": 9000,
                   "listened_ms": 1000}),
        )
        .await;
    assert_eq!(status, 204);
    assert_eq!(activity(&app, "playback.listened").await.len(), 5);

    // ------------------------------------------------------------ stats
    let (expected_total,): (i64,) =
        sqlx::query_as("SELECT COALESCE(SUM(listened_ms), 0)::bigint FROM listening_sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
    let stats = app.ok("/api/v1/listening/stats?tz=America/Toronto").await;
    assert_eq!(stats["time_zone"], "America/Toronto");
    assert_eq!(stats["total_ms"], expected_total);
    assert_eq!(stats["sessions"], 5);
    assert_eq!(stats["days_listened"], 1);
    assert_eq!(stats["today_ms"], expected_total);
    assert_eq!(stats["days"].as_array().unwrap().len(), 1);
    assert_eq!(stats["days"][0]["listened_ms"], expected_total);
    let per_book = stats["books"].as_array().unwrap();
    assert_eq!(per_book[0]["title"], "Solo Book", "most listened first");
    assert_eq!(per_book[0]["sessions"], 3);
    let (status, _) = app
        .req(Method::GET, "/api/v1/listening/stats?tz=Not/AZone", None)
        .await;
    assert_eq!(status, 400);
    let utc = app.ok("/api/v1/listening/stats").await;
    assert_eq!(utc["time_zone"], "UTC");

    // History pages, newest first.
    let first_page = app.ok("/api/v1/listening/sessions?limit=4").await;
    assert_eq!(first_page["sessions"].as_array().unwrap().len(), 4);
    let before = first_page["next_before"].as_str().unwrap().to_string();
    let rest = app
        .ok(&format!(
            "/api/v1/listening/sessions?limit=4&before={}",
            urlencode(&before)
        ))
        .await;
    assert_eq!(rest["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(rest["next_before"], Value::Null);

    // ------------------------------------------- another profile's view
    let (status, kid) = app.post("/api/v1/profiles", json!({"name": "Kid"})).await;
    assert_eq!(status, 201, "{kid}");
    let kid_id = kid["id"].as_str().unwrap().to_string();
    let (status, _) = app
        .post(
            &format!("/api/v1/admin/libraries/{library_id}/access"),
            json!({"profile_id": kid_id}),
        )
        .await;
    assert_eq!(status, 204);
    app.select(&kid_id).await;
    let theirs = app.ok("/api/v1/listening/sessions").await;
    assert_eq!(theirs["sessions"], json!([]), "sessions are per profile");
    let their_stats = app.ok("/api/v1/listening/stats").await;
    assert_eq!(their_stats["total_ms"], 0);
    assert_eq!(their_stats["books"], json!([]));
    // An admin sees everyone's, and can narrow to one profile.
    let all = app.ok("/api/v1/admin/listening/sessions").await;
    assert_eq!(all["sessions"].as_array().unwrap().len(), 5);
    assert_eq!(all["sessions"][0]["profile_name"], "jordan");
    let kid_only = app
        .ok(&format!(
            "/api/v1/admin/listening/sessions?profile_id={kid_id}"
        ))
        .await;
    assert_eq!(kid_only["sessions"], json!([]));
    app.select(&jordan_id).await;

    // ------------------------------------------------------- the shelves
    let progress = |id: &str| format!("/api/v1/audiobooks/{id}/progress");
    // Solo: in progress (AB4's progress write, unchanged).
    app.put(&progress(&solo), json!({"position_ms": 9000}))
        .await;
    // Saga One finished: Listen Again, and Saga continues with book 2.
    app.put(&progress(&saga_one), json!({"finished": true}))
        .await;
    // Another: in progress, then hidden.
    app.put(&progress(&another), json!({"position_ms": 5000}))
        .await;

    let shelves = app.ok("/api/v1/audiobooks/home").await;
    assert_eq!(
        titles(&shelves["continue_listening"]),
        ["Another", "Solo Book"],
        "in progress, the latest change first"
    );
    let cl = &shelves["continue_listening"][0];
    assert_eq!(cl["progress"]["position_ms"], 5000);
    assert_eq!(cl["duration_ms"], 30000);
    assert_eq!(titles(&shelves["listen_again"]), ["Saga One"]);
    let cs = shelves["continue_series"].as_array().unwrap();
    assert_eq!(cs.len(), 1, "{cs:?}");
    assert_eq!(cs[0]["series_name"], "Saga");
    assert_eq!(cs[0]["book"]["title"], "Saga Two");
    assert_eq!(
        titles(&shelves["discover"]),
        ["Tri One"],
        "not started, standalone or the first of a series nobody started"
    );
    assert_eq!(
        sorted(titles(&shelves["recently_added"])),
        sorted(
            [
                "Another",
                "Saga One",
                "Saga Three",
                "Saga Two",
                "Solo Book",
                "Tri One",
                "Tri Two"
            ]
            .map(String::from)
            .to_vec()
        )
    );
    assert_eq!(
        sorted(names(&shelves["recent_series"])),
        ["Saga", "Trilogy"]
    );
    let saga_card = shelves["recent_series"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "Saga")
        .unwrap();
    assert_eq!(saga_card["book_count"], 3);
    assert_eq!(saga_card["finished_count"], 1);
    assert_eq!(
        sorted(names(&shelves["newest_authors"])),
        ["Ann Writer", "Jane Doe"]
    );

    // Hide: off Continue Listening, nothing else moves.
    let hidden = app.put(&progress(&another), json!({"hidden": true})).await;
    assert_eq!(hidden["hidden"], true);
    assert_eq!(hidden["position_ms"], 5000);
    let shelves = app.ok("/api/v1/audiobooks/home").await;
    assert_eq!(titles(&shelves["continue_listening"]), ["Solo Book"]);
    assert!(
        !titles(&shelves["discover"]).contains(&"Another".to_string()),
        "a hidden book in progress is still started"
    );
    // Show it again by hand...
    app.put(&progress(&another), json!({"hidden": false})).await;
    let shelves = app.ok("/api/v1/audiobooks/home").await;
    assert_eq!(
        titles(&shelves["continue_listening"]),
        ["Another", "Solo Book"]
    );
    // ...or hide it, and the next move brings it back (AB11's rule).
    app.put(&progress(&another), json!({"hidden": true})).await;
    let moved = app
        .put(&progress(&another), json!({"position_ms": 6000}))
        .await;
    assert_eq!(moved["hidden"], false);
    let shelves = app.ok("/api/v1/audiobooks/home").await;
    assert_eq!(
        titles(&shelves["continue_listening"]),
        ["Another", "Solo Book"]
    );

    // Saga Two in progress: the series leaves Continue Series (its book
    // is on Continue Listening instead).
    app.put(&progress(&saga_two), json!({"position_ms": 3000}))
        .await;
    let shelves = app.ok("/api/v1/audiobooks/home").await;
    assert_eq!(shelves["continue_series"], json!([]));
    assert_eq!(
        titles(&shelves["continue_listening"]),
        ["Saga Two", "Another", "Solo Book"]
    );
    // Finished: Listen Again (latest first), and the series goes on
    // with book 3.
    app.put(&progress(&saga_two), json!({"finished": true}))
        .await;
    let shelves = app.ok("/api/v1/audiobooks/home").await;
    assert_eq!(titles(&shelves["listen_again"]), ["Saga Two", "Saga One"]);
    assert_eq!(
        titles(&shelves["continue_listening"]),
        ["Another", "Solo Book"]
    );
    assert_eq!(shelves["continue_series"][0]["book"]["title"], "Saga Three");

    // Scoped to one library: the same here; another profile has no
    // progress of its own, so its personal shelves are empty.
    let scoped = app
        .ok(&format!("/api/v1/audiobooks/home?library_id={library_id}"))
        .await;
    assert_eq!(
        titles(&scoped["continue_listening"]),
        ["Another", "Solo Book"]
    );
    let other = uuid::Uuid::new_v4();
    let none = app
        .ok(&format!("/api/v1/audiobooks/home?library_id={other}"))
        .await;
    assert_eq!(none["recently_added"], json!([]));
    let (status, _) = app
        .req(Method::GET, "/api/v1/audiobooks/home?library_id=nope", None)
        .await;
    assert_eq!(status, 400);
    app.select(&kid_id).await;
    let kids = app.ok("/api/v1/audiobooks/home").await;
    assert_eq!(kids["continue_listening"], json!([]));
    assert_eq!(kids["listen_again"], json!([]));
    assert_eq!(kids["continue_series"], json!([]));
    assert_eq!(kids["recently_added"].as_array().unwrap().len(), 7);
    assert_eq!(
        sorted(titles(&kids["discover"])),
        ["Another", "Saga One", "Solo Book", "Tri One"],
        "nothing started: standalones and each series' first book"
    );
    app.select(&jordan_id).await;

    // A profile with no access to the library gets nothing.
    let (status, lonely) = app.post("/api/v1/profiles", json!({"name": "Guest"})).await;
    assert_eq!(status, 201);
    app.select(lonely["id"].as_str().unwrap()).await;
    let empty = app.ok("/api/v1/audiobooks/home").await;
    assert_eq!(empty["recently_added"], json!([]));
    assert_eq!(empty["newest_authors"], json!([]));
    assert_eq!(empty["recent_series"], json!([]));
    let (status, _) = app
        .post(
            "/api/v1/playback/start",
            json!({"item_id": solo, "position_ms": 0}),
        )
        .await;
    assert_eq!(status, 404, "no access, no session");
    app.select(&jordan_id).await;

    // ------------------------------------------------ Home Screen rails
    let me = app.ok("/api/v1/auth/me").await;
    let rails: Vec<String> = me["active_profile"]["home_rails"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        rails[5..],
        [
            "continue_listening",
            "continue_series",
            "recently_added_books",
            "recent_series",
            "discover_books",
            "listen_again",
            "newest_authors"
        ]
    );
    let (status, v) = app
        .req(
            Method::PATCH,
            &format!("/api/v1/profiles/{jordan_id}"),
            Some(json!({"home_rails": [{"id": "continue_listening", "visible": false}]})),
        )
        .await;
    assert_eq!(status, 400, "Continue Listening can't be hidden: {v}");
    let (status, v) = app
        .req(
            Method::PATCH,
            &format!("/api/v1/profiles/{jordan_id}"),
            Some(json!({"home_rails": [
                {"id": "continue_listening", "visible": true},
                {"id": "newest_authors", "visible": false}
            ]})),
        )
        .await;
    assert_eq!(status, 200, "{v}");
    let me = app.ok("/api/v1/auth/me").await;
    let first = &me["active_profile"]["home_rails"][0];
    assert_eq!(first["id"], "continue_listening");
    let hidden_rail = me["active_profile"]["home_rails"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "newest_authors")
        .unwrap();
    assert_eq!(hidden_rail["visible"], false);

    // Still no play count anywhere for a book.
    let (book_rows,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM user_data u JOIN media_items i ON i.id = u.item_id
         WHERE i.kind = 'audiobook'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(book_rows, 0);

    let _ = std::fs::remove_dir_all(&root);
}

fn urlencode(s: &str) -> String {
    s.replace(':', "%3A").replace('+', "%2B")
}
