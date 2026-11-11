//! End-to-end test of the Milestone 1.5 read APIs: library item
//! filters/sorts, home rails (continue watching + recently added,
//! driven by `user_data` rows until playback reporting lands in
//! 1.6), FTS search (titles + people), and the admin users list.
//!
//! Ignored by default like the other integration tests; needs
//! PARLOUR_TEST_DATABASE_URL pointing at a throwaway database.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test web_api -- --ignored

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;

use reqwest::header::{COOKIE, SET_COOKIE};
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn make_video(dir: &std::path::Path, name: &str) {
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
            "color=c=navy:s=320x240:d=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2",
            "-shortest",
            path.to_str().unwrap(),
        ])
        .status()
        .expect("ffmpeg available");
    assert!(status.success(), "ffmpeg fixture failed");
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
async fn web_api_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test database url");
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let pool = parlour_db::connect(&db_url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");

    let fixtures = PathBuf::from(format!("/tmp/parlour-webapi-fixtures-{port}"));
    let _ = std::fs::remove_dir_all(&fixtures);
    let root_a = fixtures.join("Alpha (2001)");
    make_video(&root_a, "Alpha (2001).mkv");
    let root_b = fixtures.join("Beta (2002)");
    make_video(&root_b, "Beta (2002).mkv");
    let root_c = fixtures.join("Test Show");
    make_video(&root_c.join("Season 1"), "Test Show S01E01.mkv");

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
    let (_, me) = app.req(reqwest::Method::GET, "/api/v1/auth/me", None).await;
    assert_eq!(me["active_profile"]["id"], profile_id);

    let (status, lib) = app
        .req(
            reqwest::Method::POST,
            "/api/v1/admin/libraries",
            Some(json!({
                "name": "Movies",
                "kind": "movies",
                "paths": [fixtures.to_str().unwrap()]
            })),
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
    let job_id = job["id"].as_str().unwrap().to_string();

    // Spawn the background worker the way main.rs does.
    parlour_server::catalog::spawn_background(
        pool.clone(),
        parlour_server::metadata::MetadataSettings::default(),
        parlour_catalog::EmbySettings::default(),
    );
    wait_for_job(&app, &job_id).await;

    // Baseline list: 3 top-level items sorted by sort_title.
    let (_, items) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/libraries/{library_id}/items"),
            None,
        )
        .await;
    let items = items.as_array().unwrap();
    assert_eq!(items.len(), 3, "{items:?}");
    let alpha_id = items.iter().find(|i| i["title"] == "Alpha").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let beta_id = items.iter().find(|i| i["title"] == "Beta").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Sort by year desc: Beta (2002) first.
    let (_, by_year) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/libraries/{library_id}/items?sort=year"),
            None,
        )
        .await;
    assert_eq!(by_year[0]["title"], "Beta");
    // Year filter.
    let (_, only_2001) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/libraries/{library_id}/items?year=2001"),
            None,
        )
        .await;
    assert_eq!(only_2001.as_array().unwrap().len(), 1);
    assert_eq!(only_2001[0]["title"], "Alpha");
    // Genre filter: stamp genres via SQL, then filter on them.
    sqlx::query("UPDATE media_items SET genres = ARRAY['Drama'] WHERE id = $1")
        .bind(uuid::Uuid::parse_str(&alpha_id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let (_, dramas) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/libraries/{library_id}/items?genre=Drama"),
            None,
        )
        .await;
    assert_eq!(dramas.as_array().unwrap().len(), 1);
    assert_eq!(dramas[0]["title"], "Alpha");

    // Watch state: Beta half-watched, Alpha fully played.
    for (item, position, duration, played) in [
        (beta_id.as_str(), 60_000i64, 120_000i64, false),
        (alpha_id.as_str(), 120_000, 120_000, true),
    ] {
        sqlx::query(
            "INSERT INTO user_data (profile_id, item_id, position_ms, duration_ms, played, last_played_at)
             VALUES ($1, $2, $3, $4, $5, now())",
        )
        .bind(uuid::Uuid::parse_str(&profile_id).unwrap())
        .bind(uuid::Uuid::parse_str(item).unwrap())
        .bind(position)
        .bind(duration)
        .bind(played)
        .execute(&pool)
        .await
        .unwrap();
    }

    // Home: continue watching has Beta only; recently added has all 3.
    let (_, home) = app.req(reqwest::Method::GET, "/api/v1/home", None).await;
    let cw = home["continue_watching"].as_array().unwrap();
    assert_eq!(cw.len(), 1, "{home}");
    assert_eq!(cw[0]["title"], "Beta");
    assert_eq!(cw[0]["position_ms"], 60_000);
    assert_eq!(home["recently_added"].as_array().unwrap().len(), 3);

    // Unwatched filter drops the played Alpha.
    let (_, unwatched) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/libraries/{library_id}/items?unwatched=true"),
            None,
        )
        .await;
    let titles: Vec<&str> = unwatched
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap())
        .collect();
    assert!(!titles.contains(&"Alpha"), "{titles:?}");
    assert!(titles.contains(&"Beta"));

    // Search by title (FTS) and by person.
    let (_, found) = app
        .req(reqwest::Method::GET, "/api/v1/search?q=Alpha", None)
        .await;
    assert_eq!(found["items"].as_array().unwrap().len(), 1);
    assert_eq!(found["items"][0]["title"], "Alpha");

    let (person_id,): (uuid::Uuid,) =
        sqlx::query_as("INSERT INTO people (name) VALUES ('Ada Lovelace') RETURNING id")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO item_people (item_id, person_id, role, sort_order) VALUES ($1, $2, 'actor', 0)",
    )
    .bind(uuid::Uuid::parse_str(&beta_id).unwrap())
    .bind(person_id)
    .execute(&pool)
    .await
    .unwrap();
    let (_, found) = app
        .req(reqwest::Method::GET, "/api/v1/search?q=Lovelace", None)
        .await;
    let people = found["people"].as_array().unwrap();
    assert_eq!(people.len(), 1, "{found}");
    assert_eq!(people[0]["items"][0]["title"], "Beta");

    // Item detail carries cast + similar-by-genre (Alpha has Drama).
    sqlx::query("UPDATE media_items SET genres = ARRAY['Drama'] WHERE id = $1")
        .bind(uuid::Uuid::parse_str(&beta_id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let (_, detail) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/items/{beta_id}"),
            None,
        )
        .await;
    assert_eq!(detail["cast"][0]["name"], "Ada Lovelace");
    let similar_titles: Vec<&str> = detail["similar"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap())
        .collect();
    assert!(similar_titles.contains(&"Alpha"), "{similar_titles:?}");

    // Admin users list.
    let (status, users) = app
        .req(reqwest::Method::GET, "/api/v1/admin/users", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(users[0]["username"], "jordan");
    assert_eq!(users[0]["profile_count"], 1);

    let _ = std::fs::remove_dir_all(&fixtures);
    server.abort();
}
