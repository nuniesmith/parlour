//! XFER3: the import's rules that need no database — which copy of an
//! item's metadata wins, where imported artwork goes, parents before
//! children, the admin's options, and extracting the bundle's artwork
//! with its hash check. The walk against Postgres is
//! `tests/import_flow.rs`.
//!
//! Everything here is the fictional XFER1 fixture bundle
//! (`tests/fixtures/bundle-v1/`); no real hash or secret is involved.

use std::collections::HashMap;
use std::io::{BufWriter, Write};

use chrono::{TimeZone, Utc};
use sha2::{Digest, Sha256};

use super::apply::{artwork_dest, bundle_metadata_is_newer, parents_first, variant_is_file_safe};
use super::*;
use crate::bundle::{
    artwork_file_name, BundleWriter, CatalogFile, LibrariesFile, Manifest, ProfilesFile,
    UserDataFile, UsersFile,
};

fn fixture() -> Bundle {
    fn parse<T: serde::de::DeserializeOwned>(text: &str) -> T {
        serde_json::from_str(text).expect("fixture parses")
    }
    Bundle {
        manifest: parse::<Manifest>(include_str!("../../tests/fixtures/bundle-v1/manifest.json")),
        users: parse::<UsersFile>(include_str!("../../tests/fixtures/bundle-v1/users.json")),
        profiles: parse::<ProfilesFile>(include_str!(
            "../../tests/fixtures/bundle-v1/profiles.json"
        )),
        libraries: parse::<LibrariesFile>(include_str!(
            "../../tests/fixtures/bundle-v1/libraries.json"
        )),
        catalog: parse::<CatalogFile>(include_str!("../../tests/fixtures/bundle-v1/catalog.json")),
        user_data: parse::<UserDataFile>(include_str!(
            "../../tests/fixtures/bundle-v1/user_data.json"
        )),
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("parlour-import-{name}-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_bundles_metadata_wins_only_when_matched_more_recently() {
    let early = Some(Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap());
    let late = Some(Utc.with_ymd_and_hms(2026, 11, 2, 0, 0, 0).unwrap());
    assert!(bundle_metadata_is_newer(late, early));
    assert!(bundle_metadata_is_newer(early, None), "never matched here");
    assert!(!bundle_metadata_is_newer(early, late));
    assert!(
        !bundle_metadata_is_newer(early, early),
        "a re-import changes nothing"
    );
    assert!(!bundle_metadata_is_newer(None, early));
    assert!(
        !bundle_metadata_is_newer(None, None),
        "both unmatched: the scan here stays"
    );
}

#[test]
fn imported_artwork_lands_in_the_cache_under_a_safe_name() {
    let sha256 = format!("{:x}", Sha256::digest(b"fixture"));
    let file = artwork_file_name(&sha256, "jpg").unwrap();
    let item = Uuid::from_u128(7);
    let dest = artwork_dest(Path::new("/artwork"), item, "poster", "w300", &file).unwrap();
    assert_eq!(
        dest,
        PathBuf::from(format!("/artwork/{item}/poster-w300-{}.jpg", &sha256[..16]))
    );
    // A variant that could climb out of the folder, or is just odd,
    // travels as a provider reference only.
    for variant in ["", "../../etc", "a/b", "w300 ", &"x".repeat(33)] {
        assert!(!variant_is_file_safe(variant), "{variant:?}");
        assert!(artwork_dest(Path::new("/artwork"), item, "poster", variant, &file).is_none());
    }
    assert!(variant_is_file_safe("original") && variant_is_file_safe("w_1280-x"));
    // Not an artwork name: nothing to place.
    assert!(artwork_dest(
        Path::new("/artwork"),
        item,
        "poster",
        "w300",
        "artwork/x.jpg"
    )
    .is_none());
}

#[test]
fn parents_come_before_their_children() {
    let mut bundle = fixture();
    bundle.catalog.items.reverse();
    let order = parents_first(&bundle);
    assert_eq!(order.len(), bundle.catalog.items.len());
    let position: HashMap<(&str, &str), usize> = order
        .iter()
        .enumerate()
        .map(|(pos, &i)| {
            let item = &bundle.catalog.items[i];
            ((item.library.as_str(), item.stable_key.as_str()), pos)
        })
        .collect();
    let mut with_parent = 0;
    for item in &bundle.catalog.items {
        if let Some(parent) = &item.parent {
            with_parent += 1;
            assert!(
                position[&(item.library.as_str(), parent.as_str())]
                    < position[&(item.library.as_str(), item.stable_key.as_str())],
                "{} before its parent",
                item.stable_key
            );
        }
    }
    assert!(with_parent >= 3, "the fixture has seasons and episodes");
}

#[test]
fn options_name_bundle_libraries_and_absolute_folders() {
    let bundle = fixture();
    let with = |name: &str, path: &str| ImportOptions {
        library_paths: [(name.to_string(), vec![path.to_string()])].into(),
    };
    assert!(ImportOptions::default().check(&bundle).is_ok());
    assert!(with("Films", "/srv/media/films").check(&bundle).is_ok());
    let err = with("Cartoons", "/srv/cartoons")
        .check(&bundle)
        .unwrap_err();
    assert!(err.contains("no library named 'Cartoons'"), "{err}");
    for path in ["films", "", "  "] {
        let err = with("Films", path).check(&bundle).unwrap_err();
        assert!(err.contains("must be absolute"), "{err}");
    }
    // The request body is strict, like the export's.
    assert!(serde_json::from_str::<ImportOptions>(r#"{"library_path": {}}"#).is_err());
    let parsed: ImportOptions =
        serde_json::from_str(r#"{"library_paths": {"Films": ["/srv/films"]}}"#).unwrap();
    assert_eq!(parsed, with("Films", "/srv/films"));
    assert_eq!(
        serde_json::from_str::<ImportOptions>("{}").unwrap(),
        ImportOptions::default()
    );
}

#[test]
fn artwork_is_extracted_by_name_and_checked() {
    let dir = scratch("stage");
    let mut bundle = fixture();
    bundle.manifest.artwork_included = true;
    let poster: &[u8] = b"fixture artwork bytes, not really a jpeg";
    let sha256 = format!("{:x}", Sha256::digest(poster));
    let name = artwork_file_name(&sha256, "jpg").unwrap();
    bundle.catalog.items[0].images[0].file = Some(name.clone());
    bundle.catalog.items[0].images[1].file = Some(name.clone());

    let zip = dir.join("bundle.zip");
    let file = std::fs::File::create(&zip).unwrap();
    let mut writer = BundleWriter::new(BufWriter::new(file), &bundle).unwrap();
    writer.add_artwork(&name, &mut &poster[..]).unwrap();
    writer.finish().unwrap().flush().unwrap();

    let (mut reader, opened) = open_bundle(&zip).expect("opens");
    assert_eq!(opened, bundle);
    let staging = dir.join("staging");
    let (staged, errors) = stage_artwork(&mut reader, &staging).unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(staged.len(), 1, "one file, named twice");
    assert_eq!(std::fs::read(&staged[&name]).unwrap(), poster);
    assert_eq!(staged[&name], staging.join(format!("{sha256}.jpg")));

    // A bundle with no artwork extracts nothing and makes no folder.
    let plain = dir.join("plain.zip");
    let file = std::fs::File::create(&plain).unwrap();
    let mut bare = fixture();
    bare.manifest.artwork_included = false;
    BundleWriter::new(BufWriter::new(file), &bare)
        .unwrap()
        .finish()
        .unwrap()
        .flush()
        .unwrap();
    let (mut reader, _) = open_bundle(&plain).unwrap();
    let empty = dir.join("empty");
    let (staged, errors) = stage_artwork(&mut reader, &empty).unwrap();
    assert!(staged.is_empty() && errors.is_empty() && !empty.exists());

    // Not a bundle: refused with the reader's reason.
    let junk = dir.join("junk.zip");
    std::fs::write(&junk, b"not a zip at all").unwrap();
    assert!(open_bundle(&junk).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn uploads_and_staging_live_under_the_cache() {
    let dir = import_dir(Path::new("/cache"));
    assert_eq!(dir, PathBuf::from("/cache/imports"));
    let id = Uuid::from_u128(1);
    assert_eq!(upload_path(&dir, id), dir.join(format!("{id}.zip")));
    assert_eq!(staging_dir(&dir, id), dir.join(format!("{id}.artwork")));
    const { assert!(MAX_UPLOAD_BYTES >= 1024 * 1024 * 1024) };
}

#[tokio::test]
async fn an_upload_over_the_limit_is_refused() {
    let dir = scratch("upload");
    let dest = dir.join("upload.zip.part");
    let size = save_body(Body::from(vec![7u8; 1000]), &dest, 1000)
        .await
        .expect("at the limit is fine");
    assert_eq!(size, 1000);
    assert_eq!(std::fs::read(&dest).unwrap().len(), 1000);
    let err = save_body(Body::from(vec![7u8; 1001]), &dest, 1000)
        .await
        .expect_err("one byte over");
    assert!(err.to_string().contains("larger than 1000 bytes"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
