# Working on Parlour — notes for agents

Parlour is a clean-room, MIT-licensed media server (Rust +
Postgres + embedded SvelteKit UI) replacing the owner's Emby.
**Never copy code, assets, or wording from Emby, Jellyfin,
or Silo** — behavioural parity and similar information
architecture are fine; their code and branding are not.

## Start here
- `TODO.md` — the living checklist; work items top-down by
  phase, check boxes off in the same PR that does the work.
- `docs/DECISIONS.md` — settled judgment calls (D1–D15).
  Don't relitigate; if one must change, say so in the PR.
- `docs/ROADMAP.md`, `docs/ARCHITECTURE.md`,
  `docs/WEBUI-EMBY-NOTES.md` (UI spec for Phase 7),
  `docs/CUTOVER.md` (production deploy, pending).

## Where work happens
- Canonical clone: **`~/parlour-work` on the host `oryx`**
  (outside `~/github`, so the hourly gh-sync never touches
  it). Develop there, in a branch per PR.
- A **live test instance** runs from that same tree via
  docker compose (project `parlour-work`, containers
  `parlour-work-parlour-1` / `parlour-work-db-1`), reachable
  at https://parlour.7gram.xyz (princess reverse-proxies to
  oryx:8585). **Do not `docker compose down` that project,
  do not edit its `.env`, and don't restart it unless the
  task is deploying to it.** `.env` holds real secrets
  (DB password, TMDB key, stream secret) — never print,
  commit, or paste their values anywhere.
- Test media lives in `~/parlour-media/` on oryx, mounted
  read-only at `/media` in the container.

## Verify before you call anything done
- `cargo fmt --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace`
  (unit). Web: `npm run check` (svelte-check 0/0) +
  `npm run build` in `web/`.
- Integration tests in `crates/server/tests/*_flow.rs` are
  `#[ignore]`d and need `PARLOUR_TEST_DATABASE_URL` pointing
  at a throwaway Postgres 17. **Each flow needs its own
  freshly created database** (they collide on seeded names
  otherwise). Remove throwaway containers when finished.
- A PR is **ready only when its own CI is green on its own
  head**. Known exception: GitGuardian sometimes flags the
  *fake fixture passwords* in test files — that is a false
  positive (verify with `gitleaks` and say so on the PR);
  triage lives in the owner's GitGuardian dashboard.

## Traps already paid for once
- sqlx: **literal SQL strings only** — `query_as` rejects
  dynamically built SQL. Follow the existing query style.
- After adding a migration file, `touch crates/db/src/lib.rs`
  or local incremental builds may not pick it up
  (CI is unaffected).
- docker compose: if you override `POSTGRES_PASSWORD`, set
  `PARLOUR_DATABASE_URL` to the *same* password in the same
  env, or the app can't log into its own database.
- First-admin bootstrap (`PARLOUR_ADMIN_USER` /
  `PARLOUR_ADMIN_PASSWORD`) only fires when **zero** users
  exist. Changing the env later does nothing to real users.
- Session cookie is `Secure` on the test instance — test
  logins over the https name, not the raw port.
- Ports are Parlour's own: **8585** HTTP, 8586 reserved.
  Not Emby's 8096.
- Media mounts are read-only by default; anything that
  writes next to media (sidecars, Phase 8) needs the
  per-library opt-in designed there — don't just flip the
  mount.

## House rules
- Branch + PR for everything; the owner merges. Author
  commits as yourself if you're an agent session, and say
  plainly in the PR body what was verified and how.
- Every merged feature updates `TODO.md` checkboxes and
  `docs/` in the same PR. A new judgment call gets a
  DECISIONS entry (next D number).
- Secrets never appear in commits, PRs, logs, or chat —
  presence and length only.
