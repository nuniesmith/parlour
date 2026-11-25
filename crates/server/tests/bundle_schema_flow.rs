//! XFER1: the export bundle format against the real, current schema.
//!
//! A freshly migrated database must hold exactly the tables and
//! columns `bundle::coverage::SCHEMA` accounts for, and each CHECK
//! constraint the format mirrors must allow exactly the format's
//! values. So a migration that adds a table, a column or a value
//! fails here until the format and `docs/IMPORT-EXPORT.md` say where
//! it goes — the bundle can't silently fall behind the schema.
//!
//! Ignored by default: set PARLOUR_TEST_DATABASE_URL to a throwaway
//! database and run `cargo test -- --ignored`.

use std::collections::{BTreeMap, BTreeSet};

use parlour_server::bundle::coverage::{check_constraints, Table, SCHEMA};

#[tokio::test]
#[ignore]
async fn the_bundle_format_accounts_for_every_table_and_column() {
    let url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("PARLOUR_TEST_DATABASE_URL");
    let pool = parlour_db::connect(&url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT c.table_name::text, c.column_name::text
         FROM information_schema.columns c
         JOIN information_schema.tables t
           ON t.table_schema = c.table_schema AND t.table_name = c.table_name
         WHERE c.table_schema = 'public' AND t.table_type = 'BASE TABLE'
         ORDER BY c.table_name, c.ordinal_position",
    )
    .fetch_all(&pool)
    .await
    .expect("columns");
    let mut database: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (table, column) in rows {
        database.entry(table).or_default().insert(column);
    }
    assert!(
        database.contains_key("users") && database.contains_key("media_items"),
        "the migrations did not run: {database:?}"
    );

    let listed: BTreeSet<&str> = SCHEMA.iter().map(Table::name).collect();
    let actual: BTreeSet<&str> = database.keys().map(String::as_str).collect();
    let unlisted: Vec<&&str> = actual.difference(&listed).collect();
    let vanished: Vec<&&str> = listed.difference(&actual).collect();
    assert!(
        unlisted.is_empty(),
        "tables the bundle format doesn't account for: {unlisted:?}"
    );
    assert!(
        vanished.is_empty(),
        "tables the bundle format lists but the schema no longer has: {vanished:?}"
    );

    let mut carried = 0;
    for table in SCHEMA {
        if let Table::Carried { name, columns } = table {
            let listed: BTreeSet<&str> = columns.iter().map(|(column, _)| *column).collect();
            let actual: BTreeSet<&str> = database[*name].iter().map(String::as_str).collect();
            assert_eq!(
                listed, actual,
                "{name}: the columns the format accounts for vs the database's"
            );
            carried += 1;
        }
    }
    println!(
        "{} tables in the schema, {carried} carried by the bundle; every column accounted for",
        database.len()
    );

    for (table, constraint, values) in check_constraints() {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT pg_get_constraintdef(c.oid)
             FROM pg_constraint c
             JOIN pg_class r ON r.oid = c.conrelid
             WHERE r.relname = $1 AND c.conname = $2 AND c.contype = 'c'",
        )
        .bind(table)
        .bind(constraint)
        .fetch_optional(&pool)
        .await
        .expect("constraint");
        let (definition,) = row.unwrap_or_else(|| panic!("no CHECK {constraint} on {table}"));
        // The quoted literals: CHECK ((kind = ANY (ARRAY['movies'::text, …])))
        let allowed: BTreeSet<&str> = definition.split('\'').skip(1).step_by(2).collect();
        let format: BTreeSet<&str> = values.into_iter().collect();
        assert_eq!(
            format, allowed,
            "{table} {constraint}: the format's values vs the CHECK ({definition})"
        );
        println!("{constraint}: {} values, same as the format", allowed.len());
    }
}
