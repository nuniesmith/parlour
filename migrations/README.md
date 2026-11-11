# Migrations

Timestamped SQL files, append-only, never renumber or edit one that
has run. Applied by the server at startup via `sqlx::migrate!()`.

Naming: `YYYYMMDDHHMMSS_description.sql`
