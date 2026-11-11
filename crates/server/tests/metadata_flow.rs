//! End-to-end metadata flow (MD1–MD4) against a real Postgres
//! and a **stub TMDB server** — no real API key, no internet:
//! the client's base URL points at a tiny local axum app serving
//! fixture JSON + fake poster bytes.
//!
//! Covers: auto-match (exact title+year), needs_review (two
//! equal-scoring candidates — never auto-guessed), unmatched,
//! series + episode enrichment, review queue, manual apply,
//! artwork download at two sizes, generated tile for a posterless
//! item, and artwork serving with library access.
//!
//! Ignored by default: set PARLOUR_TEST_DATABASE_URL.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

use axum::extract::{Path as AxumPath, Query};
use axum::routing::get;
use axum::{Json, Router};
use parlour_server::config::Config;
use parlour_server::http::AppState;
use serde_json::{json, Value};

// ------------------------------------------------------------ stub TMDB

async fn stub_search_movie(Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let results = match q.get("query").map(String::as_str) {
        Some("Test Movie") => json!([
            {"id": 101, "title": "Test Movie", "release_date": "2021-05-01",
             "overview": "A test.", "poster_path": "/poster101.jpg", "vote_average": 7.5}
        ]),
        Some("Ambiguous Tale") => json!([
            {"id": 201, "title": "Ambiguous Tale", "release_date": "2020-01-01"},
            {"id": 202, "title": "Ambiguous Tale", "release_date": "2020-06-01"}
        ]),
        Some("Obscure Thing") => json!([
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
             "overview": "A show.", "poster_path": "/poster301.jpg", "vote_average": 8.1}
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
            "poster_path": "/poster101.jpg", "backdrop_path": "/backdrop101.jpg",
            "genres": [{"name": "Drama"}],
            "external_ids": {"imdb_id": "tt0000101", "tvdb_id": null},
            "release_dates": {"results": [
                {"iso_3166_1": "US", "release_dates": [{"certification": "PG"}]}
            ]},
            "credits": {"cast": [
                {"name": "Ada Actor", "character": "Lead", "order": 0, "id": 501}
            ]}
        }),
        201 | 202 => json!({
            "id": id, "title": "Ambiguous Tale", "release_date": "2020-01-01",
            "overview": "One of them.", "runtime": 88, "vote_average": 6.0,
            "poster_path": "/poster201.jpg",
            "genres": [{"name": "Comedy"}],
            "external_ids": {}, "release_dates": {"results": []},
            "credits": {"cast": []}
        }),
        _ => json!({"id": id, "title": "Stub"}),
    })
}

async fn stub_tv(AxumPath(id): AxumPath<i64>) -> Json<Value> {
    Json(json!({
        "id": id, "name": "Test Show", "first_air_date": "2020-03-01",
        "overview": "A show.", "episode_run_time": [42], "vote_average": 8.1,
        "poster_path": "/poster301.jpg",
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
         "overview": "It begins.", "air_date": "2020-03-01", "runtime": 42,
         "still_path": "/still301.jpg"}
    ]}))
}

async fn stub_image(AxumPath((size, file)): AxumPath<(String, String)>) -> Vec<u8> {
    let _ = (size, file);
    b"FAKEJPEGDATA".to_vec()
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
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn login(&self) -> (String, Value) {
        let res = self
            .http
            .request(
                reqwest::Method::POST,
                format!("{}/api/v1/auth/login", self.base),
            )
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

    async fn wait_job(&self, cookie: &str, job_id: &str) -> Value {
        for _ in 0..80 {
            let (_, job) = self
                .req(
                    reqwest::Method::GET,
                    &format!("/api/v1/admin/jobs/{job_id}"),
                    Some(cookie),
                    None,
                )
                .await;
            match job["status"].as_str() {
                Some("succeeded") => return job,
                Some("failed") => panic!("job failed: {job}"),
                _ => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
            }
        }
        panic!("job did not finish");
    }
}

fn make_media(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let ok = Command::new("ffmpeg")
        .args([
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

// ------------------------------------------------------------ the flow

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn metadata_flow_end_to_end() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let config = test_config(&db_url);
    let pool = parlour_db::connect(&config.database_url).await.unwrap();
    parlour_db::migrate(&pool).await.unwrap();
    parlour_server::auth::bootstrap_admin(&pool, &config)
        .await
        .unwrap();

    // Stub TMDB on an ephemeral port.
    let stub_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stub_addr = stub_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(stub_listener, stub_app()).await.unwrap();
    });
    let stub_base = format!("http://{stub_addr}");

    // Fixture media tree.
    let root: PathBuf = std::env::temp_dir().join(format!("parlour-meta-{}", uuid::Uuid::new_v4()));
    let media = root.join("media");
    make_media(&media.join("Test Movie (2021)/Test Movie (2021).mkv"));
    make_media(&media.join("Ambiguous Tale (2020)/Ambiguous Tale (2020).mkv"));
    make_media(&media.join("Obscure Thing (1984)/Obscure Thing (1984).mkv"));
    make_media(&media.join("Test Show/Season 1/Test Show - S01E01.mkv"));

    let artwork_dir = root.join("artwork");
    let mut state = AppState::new(pool, None, false);
    state.metadata = parlour_server::metadata::MetadataSettings {
        tmdb_api_key: Some("test-key".to_string()),
        tmdb_base: Some(stub_base),
        artwork_dir: artwork_dir.clone(),
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
    let client = Client {
        http: reqwest::Client::new(),
        base: format!("http://{addr}"),
    };

    let (cookie, me) = client.login().await;
    let profile_id = me["profiles"][0]["id"].as_str().unwrap().to_string();
    let (s, _) = client
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/profiles/{profile_id}/select"),
            Some(&cookie),
            Some(json!({})),
        )
        .await;
    assert_eq!(s, 200);

    let (s, lib) = client
        .req(
            reqwest::Method::POST,
            "/api/v1/admin/libraries",
            Some(&cookie),
            Some(json!({"name": "Mixed", "kind": "movies", "paths": [media.to_string_lossy().to_string()]})),
        )
        .await;
    assert_eq!(s, 201, "{lib}");
    let lib_id = lib["id"].as_str().unwrap().to_string();
    let (s, _) = client
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/admin/libraries/{lib_id}/access"),
            Some(&cookie),
            Some(json!({"profile_id": profile_id})),
        )
        .await;
    assert_eq!(s, 204);

    // Scan first (the catalog the matcher works on).
    let (s, job) = client
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/admin/libraries/{lib_id}/scan"),
            Some(&cookie),
            None,
        )
        .await;
    assert_eq!(s, 202);
    client.wait_job(&cookie, job["id"].as_str().unwrap()).await;

    // Match the library.
    let (s, job) = client
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/admin/libraries/{lib_id}/metadata"),
            Some(&cookie),
            None,
        )
        .await;
    assert_eq!(s, 202, "{job}");
    let job = client.wait_job(&cookie, job["id"].as_str().unwrap()).await;
    let result = &job["result"];
    assert_eq!(result["matched"], json!(2), "{result}");
    assert_eq!(result["needs_review"], json!(1), "{result}");
    assert_eq!(result["unmatched"], json!(1), "{result}");

    // Item states.
    let (_, items) = client
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/libraries/{lib_id}/items"),
            Some(&cookie),
            None,
        )
        .await;
    let by_title = |t: &str| {
        items
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["title"] == json!(t))
            .unwrap_or_else(|| panic!("no item {t}"))
            .clone()
    };
    let movie = by_title("Test Movie");
    assert_eq!(movie["metadata_status"], json!("matched"));
    assert_eq!(movie["genres"], json!(["Drama"]));
    let movie_id = movie["id"].as_str().unwrap().to_string();
    let ambig = by_title("Ambiguous Tale");
    assert_eq!(ambig["metadata_status"], json!("needs_review"));
    let ambig_id = ambig["id"].as_str().unwrap().to_string();
    assert_eq!(
        by_title("Obscure Thing")["metadata_status"],
        json!("unmatched")
    );
    let obscure_id = by_title("Obscure Thing")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let show_id = by_title("Test Show")["id"].as_str().unwrap().to_string();

    // Movie detail: overview + cast-side data + image rows exist.
    let (_, detail) = client
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/items/{movie_id}"),
            Some(&cookie),
            None,
        )
        .await;
    assert_eq!(detail["overview"], json!("A test movie."));
    assert_eq!(detail["content_rating"], json!("PG"));
    assert!(detail["images"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["kind"] == json!("poster") && i["variant"] == json!("original")));

    // Series: episode got the provider title/overview.
    let (_, show) = client
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/items/{show_id}"),
            Some(&cookie),
            None,
        )
        .await;
    let season = &show["children"][0];
    let (_, season_detail) = client
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/items/{}", season["id"].as_str().unwrap()),
            Some(&cookie),
            None,
        )
        .await;
    assert_eq!(
        season_detail["children"][0]["title"],
        json!("The Beginning")
    );
    assert_eq!(
        season_detail["children"][0]["overview"],
        json!("It begins.")
    );

    // Review queue carries both candidates for the ambiguous item.
    let (_, queue) = client
        .req(
            reqwest::Method::GET,
            "/api/v1/admin/metadata/review-queue",
            Some(&cookie),
            None,
        )
        .await;
    let entry = queue
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(ambig_id))
        .expect("review queue entry");
    assert_eq!(entry["candidates"].as_array().unwrap().len(), 2);

    // Manual fix: apply candidate 201; status flips, artwork is
    // cached by the apply flow itself.
    let (s, applied) = client
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/admin/items/{ambig_id}/metadata/apply"),
            Some(&cookie),
            Some(json!({"provider_id": 201})),
        )
        .await;
    assert_eq!(s, 200, "{applied}");
    let (_, detail) = client
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/items/{ambig_id}"),
            Some(&cookie),
            None,
        )
        .await;
    assert_eq!(detail["metadata_status"], json!("matched"));
    assert!(detail["images"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["kind"] == json!("poster")
            && i["path"].as_str().map(|p| !p.is_empty()) == Some(true)));

    // Artwork job for the movie: original + w300 cached, served.
    let (s, job) = client
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/admin/items/{movie_id}/artwork"),
            Some(&cookie),
            None,
        )
        .await;
    assert_eq!(s, 202);
    client.wait_job(&cookie, job["id"].as_str().unwrap()).await;
    let poster = artwork_dir.join(&movie_id).join("poster-original.jpg");
    let poster_small = artwork_dir.join(&movie_id).join("poster-w300.jpg");
    assert!(poster.exists(), "original poster cached");
    assert!(poster_small.exists(), "w300 poster cached");
    let res = client
        .http
        .get(format!(
            "{}/api/v1/items/{movie_id}/artwork/poster",
            client.base
        ))
        .header("Cookie", format!("parlour_session={cookie}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.bytes().await.unwrap().as_ref(), b"FAKEJPEGDATA");

    // Posterless item: generated tile instead of an error.
    let (s, job) = client
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/admin/items/{obscure_id}/artwork"),
            Some(&cookie),
            None,
        )
        .await;
    assert_eq!(s, 202);
    client.wait_job(&cookie, job["id"].as_str().unwrap()).await;
    let res = client
        .http
        .get(format!(
            "{}/api/v1/items/{obscure_id}/artwork/poster",
            client.base
        ))
        .header("Cookie", format!("parlour_session={cookie}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let ct = res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(ct.contains("svg"), "generated tile served as svg, got {ct}");

    let _ = std::fs::remove_dir_all(&root);
}
