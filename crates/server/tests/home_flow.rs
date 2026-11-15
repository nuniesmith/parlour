//! End-to-end Home flow against a real Postgres (UI8): the
//! library tile row and the per-library Latest rails on
//! `GET /api/v1/home`, and the two new rail ids on the profile.
//!
//! Ignored by default: set PARLOUR_TEST_DATABASE_URL to a
//! throwaway database and run `cargo test -- --ignored`.
//! The database is migrated fresh by the server code itself.

use std::net::SocketAddr;

use parlour_server::config::Config;
use parlour_server::http::AppState;
use serde_json::{json, Value};
use uuid::Uuid;

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
    ) -> (reqwest::StatusCode, Value, Option<String>) {
        let mut rb = self.http.request(method, format!("{}{path}", self.base));
        if let Some(c) = cookie {
            rb = rb.header("Cookie", format!("parlour_session={c}"));
        }
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let res = rb.send().await.expect("request");
        let status = res.status();
        let set_cookie = res
            .headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| {
                s.split(';')
                    .next()
                    .and_then(|kv| kv.split_once('='))
                    .map(|(_, v)| v.to_string())
            });
        let text = res.text().await.unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        (status, value, set_cookie)
    }

    async fn login(
        &self,
        username: &str,
        password: &str,
    ) -> (reqwest::StatusCode, Value, Option<String>) {
        self.req(
            reqwest::Method::POST,
            "/api/v1/auth/login",
            None,
            Some(json!({"username": username, "password": password})),
        )
        .await
    }
}

async fn library(pool: &sqlx::PgPool, name: &str, kind: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO libraries (name, kind) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(kind)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Insert an item created `hours_ago` hours in the past.
async fn item(
    pool: &sqlx::PgPool,
    library: Uuid,
    parent: Option<Uuid>,
    kind: &str,
    title: &str,
    hours_ago: i32,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO media_items (library_id, parent_id, kind, stable_key, title, sort_title, created_at)
         VALUES ($1, $2, $3, $4, $5, $5, now() - make_interval(hours => $6))
         RETURNING id",
    )
    .bind(library)
    .bind(parent)
    .bind(kind)
    .bind(format!("{kind}:{title}"))
    .bind(title)
    .bind(hours_ago)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn image(pool: &sqlx::PgPool, item: Uuid, kind: &str, path: &str) {
    sqlx::query("INSERT INTO images (item_id, kind, path) VALUES ($1, $2, $3)")
        .bind(item)
        .bind(kind)
        .bind(path)
        .execute(pool)
        .await
        .unwrap();
}

fn titles(items: &Value) -> Vec<&str> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap())
        .collect()
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn home_flow() {
    let url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let cfg = test_config(&url);
    let pool = parlour_db::connect(&url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");
    parlour_server::auth::bootstrap_admin(&pool, &cfg)
        .await
        .expect("bootstrap");

    let state = AppState::new(pool.clone(), None, false);
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

    let (status, body, cookie) = client.login("jordan", "admin-pass-123").await;
    assert_eq!(status, 200, "{body}");
    let cookie = cookie.expect("session cookie");
    let c = Some(cookie.as_str());
    let profile_str = body["profiles"][0]["id"].as_str().unwrap().to_string();
    let profile = Uuid::parse_str(&profile_str).unwrap();

    // The two UI8 rails are known to the profile: the tile row
    // first and the Latest group last, both visible by default.
    let rails: Vec<&str> = body["profiles"][0]["home_rails"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        rails,
        [
            "libraries",
            "continue_watching",
            "next_up",
            "recently_added",
            "latest"
        ]
    );

    // Home needs a profile.
    let (status, _, _) = client.req(Method::GET, "/api/v1/home", c, None).await;
    assert_eq!(status, 400);
    let (status, _, _) = client
        .req(
            Method::POST,
            &format!("/api/v1/profiles/{profile_str}/select"),
            c,
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 200);

    // Nothing granted yet: no tiles, no Latest rails.
    let (status, home, _) = client.req(Method::GET, "/api/v1/home", c, None).await;
    assert_eq!(status, 200, "{home}");
    assert_eq!(home["libraries"], json!([]));
    assert_eq!(home["latest"], json!([]));

    // Seed: Films (movies), Shows (shows), an empty library, and a
    // library this profile cannot see.
    let films = library(&pool, "Films", "movies").await;
    let shows = library(&pool, "Shows", "shows").await;
    let empty = library(&pool, "Archive", "movies").await;
    let hidden = library(&pool, "Private", "movies").await;
    for lib in [films, shows, empty] {
        sqlx::query("INSERT INTO library_access (profile_id, library_id) VALUES ($1, $2)")
            .bind(profile)
            .bind(lib)
            .execute(&pool)
            .await
            .unwrap();
    }

    let old_film = item(&pool, films, None, "movie", "Old Film", 72).await;
    let new_film = item(&pool, films, None, "movie", "New Film", 24).await;
    for n in 0..18 {
        item(
            &pool,
            films,
            None,
            "movie",
            &format!("Filler {n:02}"),
            720 + n,
        )
        .await;
    }
    // Old Film has real backdrop art; New Film's is still pending,
    // so the tile uses Old Film's.
    image(
        &pool,
        old_film,
        "backdrop",
        "/artwork/old-film-backdrop.jpg",
    )
    .await;
    image(
        &pool,
        new_film,
        "backdrop",
        "pending:https://example.invalid/b.jpg",
    )
    .await;

    // Show A is old but just got a new episode; Show B is newer
    // as a series but nothing has been added to it since.
    let show_a = item(&pool, shows, None, "series", "Show A", 240).await;
    let season = item(&pool, shows, Some(show_a), "season", "Show A Season 1", 240).await;
    item(&pool, shows, Some(season), "episode", "Show A S01E02", 1).await;
    item(&pool, shows, None, "series", "Show B", 48).await;

    item(&pool, hidden, None, "movie", "Secret Film", 0).await;

    let (status, home, _) = client.req(Method::GET, "/api/v1/home", c, None).await;
    assert_eq!(status, 200, "{home}");

    // Tile row: every visible library by name, with counts and art.
    let tiles = home["libraries"].as_array().unwrap();
    let names: Vec<&str> = tiles.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Archive", "Films", "Shows"], "{home}");
    assert_eq!(tiles[0]["item_count"], 0);
    assert_eq!(tiles[0]["art_item_id"], Value::Null);
    assert_eq!(tiles[1]["kind"], "movies");
    assert_eq!(tiles[1]["item_count"], 20);
    assert_eq!(tiles[1]["art_item_id"], old_film.to_string());
    assert_eq!(
        tiles[2]["item_count"], 2,
        "seasons/episodes are not counted"
    );
    assert_eq!(tiles[2]["art_item_id"], Value::Null);

    // Latest: one rail per non-empty library, in tile order.
    let latest = home["latest"].as_array().unwrap();
    let rail_names: Vec<&str> = latest
        .iter()
        .map(|r| r["library_name"].as_str().unwrap())
        .collect();
    assert_eq!(rail_names, ["Films", "Shows"], "{home}");
    assert_eq!(latest[0]["library_id"], films.to_string());
    assert_eq!(latest[0]["library_kind"], "movies");
    let film_titles = titles(&latest[0]["items"]);
    assert_eq!(film_titles.len(), 16, "capped per library");
    assert_eq!(&film_titles[..3], ["New Film", "Old Film", "Filler 00"]);
    // A new episode lifts its series; only top-level items appear.
    assert_eq!(titles(&latest[1]["items"]), ["Show A", "Show B"]);

    // Nothing from the library the profile cannot see, anywhere.
    let all = home.to_string();
    assert!(!all.contains("Secret Film"), "{all}");
    assert!(!all.contains("Private"), "{all}");
    assert!(!all.contains(&hidden.to_string()), "{all}");

    // The new rails can be hidden and reordered like the others.
    let (status, body, _) = client
        .req(
            Method::PATCH,
            &format!("/api/v1/profiles/{profile_str}"),
            c,
            Some(json!({"home_rails": [
                {"id": "latest", "visible": true},
                {"id": "libraries", "visible": false},
            ]})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["home_rails"][0],
        json!({"id": "latest", "visible": true})
    );
    assert_eq!(
        body["home_rails"][1],
        json!({"id": "libraries", "visible": false})
    );

    // Signed out is 401.
    let (status, _, _) = client.req(Method::GET, "/api/v1/home", None, None).await;
    assert_eq!(status, 401);
}
