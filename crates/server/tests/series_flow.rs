//! End-to-end test of AB7 (series and author browsing, D28): series
//! from the `Author/Series/Title` folder and from tags (`series` /
//! `series-part`, `grouping`), one book in two series, series pages
//! in sequence order, author pages, library access, a missing book,
//! empty series removed, and re-scans that never duplicate a series
//! or a link.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a fresh
//! database) and ffmpeg/ffprobe on PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test series_flow -- --ignored

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

/// A tagged one-second MP3.
fn make_mp3(dir: &Path, tags: &[(&str, &str)]) {
    std::fs::create_dir_all(dir).unwrap();
    let mut args: Vec<String> = [
        "-y",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for (k, v) in tags {
        args.push("-metadata".into());
        args.push(format!("{k}={v}"));
    }
    args.extend(["-c:a", "libmp3lame", "-b:a", "32k"].map(String::from));
    args.push(dir.join("01.mp3").to_str().unwrap().to_string());
    let status = Command::new("ffmpeg")
        .args(&args)
        .status()
        .expect("ffmpeg available");
    assert!(status.success(), "ffmpeg failed: {args:?}");
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

    async fn get(&self, path: &str) -> (u16, Value) {
        self.req(Method::GET, path, None).await
    }

    async fn ok(&self, path: &str) -> Value {
        let (status, v) = self.get(path).await;
        assert_eq!(status, 200, "{path}: {v}");
        v
    }
}

fn strs(v: &Value, key: &str) -> Vec<String> {
    v.as_array()
        .unwrap_or_else(|| panic!("not a list: {v}"))
        .iter()
        .map(|x| x[key].as_str().unwrap().to_string())
        .collect()
}

fn find<'a>(list: &'a Value, key: &str, value: &str) -> &'a Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|x| x[key] == value)
        .unwrap_or_else(|| panic!("no {key} = {value} in {list}"))
}

async fn counts(pool: &parlour_db::PgPool) -> (i64, i64) {
    let series: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series")
        .fetch_one(pool)
        .await
        .unwrap();
    let links: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_series")
        .fetch_one(pool)
        .await
        .unwrap();
    (series, links)
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL and ffmpeg"]
async fn series_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test database url");
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let pool = parlour_db::connect(&db_url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");

    // Library layout:
    //   Jane Doe/Road Two/                 grouping "The Long Road #2; Side Paths #1"
    //   Jane Doe/The Long Road/Road One/   series "The Long Road", part 1
    //   Jane Doe/The Long Road/Road Three/ series "the long road" (case), part 3
    //   Jane Doe/The Long Road/Interlude/  no series tags: the folder's
    //   Bo Writer/Side Paths/Path Zero/    series "Side Paths", part 0.5
    //   Loose/                             (one folder: no series)
    let root = PathBuf::from(format!("/tmp/parlour-series-fixtures-{port}"));
    let _ = std::fs::remove_dir_all(&root);
    let jane = root.join("Jane Doe");
    make_mp3(
        &jane.join("Road Two"),
        &[
            ("album", "Road Two"),
            ("album_artist", "Jane Doe"),
            ("grouping", "The Long Road #2; Side Paths #1"),
        ],
    );
    let long_road = jane.join("The Long Road");
    make_mp3(
        &long_road.join("Road One"),
        &[
            ("album", "Road One"),
            ("album_artist", "Jane Doe"),
            ("series", "The Long Road"),
            ("series-part", "1"),
        ],
    );
    make_mp3(
        &long_road.join("Road Three"),
        &[
            ("album", "Road Three"),
            ("album_artist", "Jane Doe"),
            ("series", "the long road"),
            ("series-part", "3"),
        ],
    );
    make_mp3(
        &long_road.join("Interlude"),
        &[("album", "Interlude"), ("album_artist", "Jane Doe")],
    );
    let path_zero = root.join("Bo Writer").join("Side Paths").join("Path Zero");
    make_mp3(
        &path_zero,
        &[
            ("album", "Path Zero"),
            ("album_artist", "Bo Writer"),
            ("series", "Side Paths"),
            ("series-part", "0.5"),
        ],
    );
    make_mp3(
        &root.join("Loose"),
        &[("album", "Loose"), ("artist", "Bo Writer")],
    );

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
    let (_, profiles) = app.get("/api/v1/profiles").await;
    let profile_id = profiles[0]["id"].as_str().unwrap().to_string();
    let select = |id: String| {
        let app = &app;
        async move {
            let (status, _) = app
                .req(
                    Method::POST,
                    &format!("/api/v1/profiles/{id}/select"),
                    Some(json!({})),
                )
                .await;
            assert_eq!(status, 200);
        }
    };
    select(profile_id.clone()).await;

    let (status, lib) = app
        .req(
            Method::POST,
            "/api/v1/admin/libraries",
            Some(json!({"name": "Audiobooks", "kind": "audiobooks",
                        "paths": [root.to_str().unwrap()]})),
        )
        .await;
    assert_eq!(status, 201, "{lib}");
    let library_id = lib["id"].as_str().unwrap().to_string();
    let library_uuid = uuid::Uuid::parse_str(&library_id).unwrap();
    let (status, _) = app
        .req(
            Method::POST,
            &format!("/api/v1/admin/libraries/{library_id}/access"),
            Some(json!({"profile_id": profile_id})),
        )
        .await;
    assert_eq!(status, 204);
    let scan = || parlour_catalog::scan_library(&pool, library_uuid, None);
    let report = scan().await.expect("scan");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.seen_files, 6);

    // ------------------------------------------------------- series
    let series = app
        .ok(&format!("/api/v1/series?library_id={library_id}"))
        .await;
    // Sorted without the article: "Long Road" before "Side Paths".
    assert_eq!(strs(&series, "name"), ["The Long Road", "Side Paths"]);
    let road = find(&series, "name", "The Long Road");
    assert_eq!(road["book_count"], 4, "{road}");
    assert_eq!(road["finished_count"], 0);
    assert_eq!(road["library_id"], library_id.as_str());
    let road_id = road["id"].as_str().unwrap().to_string();
    let side = find(&series, "name", "Side Paths");
    assert_eq!(side["book_count"], 2);
    let side_id = side["id"].as_str().unwrap().to_string();
    assert_eq!(counts(&pool).await, (2, 6), "one record per series");

    // A series page lists its books in sequence order; the folder-only
    // book has no sequence and comes last.
    let detail = app.ok(&format!("/api/v1/series/{road_id}")).await;
    assert_eq!(detail["name"], "The Long Road");
    assert_eq!(
        strs(&detail["books"], "title"),
        ["Road One", "Road Two", "Road Three", "Interlude"]
    );
    let interlude = find(&detail["books"], "title", "Interlude");
    assert_eq!(
        interlude["series"],
        json!([{"id": road_id, "name": "The Long Road", "sequence": null}]),
        "the series folder, no sequence"
    );
    let three = find(&detail["books"], "title", "Road Three");
    assert_eq!(
        three["series"],
        json!([{"id": road_id, "name": "The Long Road", "sequence": "3"}]),
        "a name differing only in case is the same series"
    );
    let detail = app.ok(&format!("/api/v1/series/{side_id}")).await;
    assert_eq!(strs(&detail["books"], "title"), ["Path Zero", "Road Two"]);

    // One book in two series, each with its own sequence.
    let books = app.ok("/api/v1/audiobooks").await;
    let two = find(&books, "title", "Road Two");
    assert_eq!(
        two["series"],
        json!([
            {"id": road_id, "name": "The Long Road", "sequence": "2"},
            {"id": side_id, "name": "Side Paths", "sequence": "1"},
        ])
    );
    let two_id = two["id"].as_str().unwrap().to_string();
    assert_eq!(find(&books, "title", "Loose")["series"], json!([]));
    let (_, book) = app.get(&format!("/api/v1/audiobooks/{two_id}")).await;
    assert_eq!(book["series"], two["series"], "the detail carries it too");

    // Searching: series by name, and books by their series' name.
    let found = app.ok("/api/v1/series?q=side").await;
    assert_eq!(strs(&found, "name"), ["Side Paths"]);
    let found = app.ok("/api/v1/audiobooks?q=side%20paths").await;
    assert_eq!(strs(&found, "title"), ["Path Zero", "Road Two"]);

    // Finished counts are this profile's.
    let (status, _) = app
        .req(
            Method::PUT,
            &format!("/api/v1/audiobooks/{two_id}/progress"),
            Some(json!({"finished": true})),
        )
        .await;
    assert_eq!(status, 200);
    let series = app.ok("/api/v1/series").await;
    assert_eq!(find(&series, "name", "Side Paths")["finished_count"], 1);

    let (status, _) = app.get("/api/v1/series/not-a-uuid").await;
    assert_eq!(status, 404);
    let (status, _) = app
        .get(&format!("/api/v1/series/{}", uuid::Uuid::new_v4()))
        .await;
    assert_eq!(status, 404);
    let (status, _) = app.get("/api/v1/series?library_id=nope").await;
    assert_eq!(status, 400);

    // ------------------------------------------------------ authors
    let authors = app.ok("/api/v1/authors").await;
    assert_eq!(strs(&authors, "name"), ["Bo Writer", "Jane Doe"]);
    let jane_row = find(&authors, "name", "Jane Doe");
    assert_eq!(jane_row["book_count"], 4);
    let jane_id = jane_row["id"].as_str().unwrap().to_string();
    let bo_id = find(&authors, "name", "Bo Writer")["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        two["author_refs"],
        json!([{"id": jane_id, "name": "Jane Doe"}]),
        "a book links to its authors' pages"
    );

    let page = app.ok(&format!("/api/v1/authors/{jane_id}")).await;
    assert_eq!(page["name"], "Jane Doe");
    assert_eq!(page["book_count"], 4);
    assert_eq!(
        strs(&page["books"], "title"),
        ["Interlude", "Road One", "Road Three", "Road Two"],
        "an author's books by title"
    );
    assert_eq!(
        strs(&page["series"], "name"),
        ["The Long Road", "Side Paths"]
    );
    assert_eq!(
        find(&page["series"], "name", "The Long Road")["book_count"],
        4
    );
    assert_eq!(find(&page["series"], "name", "Side Paths")["book_count"], 1);
    let page = app.ok(&format!("/api/v1/authors/{bo_id}")).await;
    assert_eq!(strs(&page["books"], "title"), ["Loose", "Path Zero"]);
    assert_eq!(strs(&page["series"], "name"), ["Side Paths"]);
    let page = app
        .ok(&format!("/api/v1/authors/{bo_id}?library_id={library_id}"))
        .await;
    assert_eq!(page["book_count"], 2);
    let found = app.ok("/api/v1/authors?q=jane").await;
    assert_eq!(strs(&found, "name"), ["Jane Doe"]);
    let (status, _) = app
        .get(&format!("/api/v1/authors/{}", uuid::Uuid::new_v4()))
        .await;
    assert_eq!(status, 404);

    // ------------------------------------------------------- access
    let (status, kid) = app
        .req(
            Method::POST,
            "/api/v1/profiles",
            Some(json!({"name": "Kid"})),
        )
        .await;
    assert_eq!(status, 201, "{kid}");
    select(kid["id"].as_str().unwrap().to_string()).await;
    assert_eq!(app.ok("/api/v1/series").await, json!([]));
    assert_eq!(app.ok("/api/v1/authors").await, json!([]));
    for path in [
        format!("/api/v1/series/{road_id}"),
        format!("/api/v1/authors/{jane_id}"),
    ] {
        let (status, _) = app.get(&path).await;
        assert_eq!(status, 404, "{path} without access");
    }
    select(profile_id.clone()).await;

    // ------------------------------------------------------ re-scans
    let report = scan().await.expect("rescan");
    assert_eq!(report.unchanged_files, 6);
    assert_eq!(report.probed_files, 0);
    assert_eq!(counts(&pool).await, (2, 6), "a re-scan duplicates nothing");

    // A new precedence merges every book again: without the folder
    // source, the folder-only book leaves its series; the series
    // records keep their ids.
    let precedence = format!("/api/v1/admin/libraries/{library_id}/metadata-precedence");
    let (status, _) = app
        .req(
            Method::PUT,
            &precedence,
            Some(json!({"sources": ["tags", "nfo", "txt", "opf", "abs_json", "sidecar"]})),
        )
        .await;
    assert_eq!(status, 200);
    let report = scan().await.expect("rescan");
    assert_eq!(report.probed_files, 6);
    assert_eq!(counts(&pool).await, (2, 5));
    let detail = app.ok(&format!("/api/v1/series/{road_id}")).await;
    assert_eq!(
        strs(&detail["books"], "title"),
        ["Road One", "Road Two", "Road Three"]
    );
    // Back to the default: the same records, the same six links, and
    // merging twice changes nothing.
    let (status, _) = app
        .req(
            Method::PUT,
            &precedence,
            Some(
                json!({"sources": ["folder", "tags", "nfo", "txt", "opf", "abs_json",
                                    "sidecar"]}),
            ),
        )
        .await;
    assert_eq!(status, 200);
    scan().await.expect("rescan");
    assert_eq!(counts(&pool).await, (2, 6));
    let again = app.ok("/api/v1/series").await;
    assert_eq!(strs(&again, "id"), [road_id.as_str(), side_id.as_str()]);
    let report = scan().await.expect("rescan");
    assert_eq!(report.probed_files, 0);
    assert_eq!(counts(&pool).await, (2, 6));

    // A missing book keeps its place, shown unavailable.
    std::fs::remove_dir_all(&path_zero).unwrap();
    let report = scan().await.expect("rescan");
    assert_eq!(report.missing_files, 1);
    let detail = app.ok(&format!("/api/v1/series/{side_id}")).await;
    assert_eq!(strs(&detail["books"], "title"), ["Path Zero", "Road Two"]);
    assert_eq!(
        find(&detail["books"], "title", "Path Zero")["available"],
        false
    );

    // A series no book names any more goes: with neither the folder
    // nor the tags read, the available books lose their series and
    // The Long Road is removed. The missing book isn't merged again,
    // so it keeps Side Paths alive.
    let (status, _) = app
        .req(
            Method::PUT,
            &precedence,
            Some(json!({"sources": ["sidecar"]})),
        )
        .await;
    assert_eq!(status, 200);
    scan().await.expect("rescan");
    assert_eq!(counts(&pool).await, (1, 1));
    assert_eq!(
        strs(&app.ok("/api/v1/series").await, "name"),
        ["Side Paths"]
    );
    let (status, _) = app.get(&format!("/api/v1/series/{road_id}")).await;
    assert_eq!(status, 404);

    let _ = std::fs::remove_dir_all(&root);
}
