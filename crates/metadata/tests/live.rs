//! Live TMDB smoke test — ignored by default, needs a real key:
//! `TMDB_API_KEY=... cargo test -p parlour-metadata -- --ignored`
//! Verifies the real API shapes the fixture tests were built from.

use parlour_metadata::tmdb::TmdbClient;
use parlour_metadata::{MediaKind, MetadataProvider};

#[tokio::test]
#[ignore = "needs TMDB_API_KEY"]
async fn live_tmdb_search_and_details() {
    let key = std::env::var("TMDB_API_KEY").expect("TMDB_API_KEY");
    let client = TmdbClient::new(key);

    let results = client
        .search(MediaKind::Movie, "Alien", Some(1979))
        .await
        .expect("search");
    let first = results.first().expect("at least one result");
    assert_eq!(first.title, "Alien");
    assert_eq!(first.year, Some(1979));

    let details = client
        .details(MediaKind::Movie, first.provider_id)
        .await
        .expect("details");
    assert_eq!(details.title, "Alien");
    assert!(details.poster_url.is_some());
    assert!(!details.cast.is_empty());

    let shows = client
        .search(MediaKind::Show, "Breaking Bad", Some(2008))
        .await
        .expect("tv search");
    let show = shows.first().expect("show result");
    let eps = client
        .season_episodes(show.provider_id, 1)
        .await
        .expect("episodes");
    assert!(!eps.is_empty());
    assert_eq!(eps[0].season, 1);
}
