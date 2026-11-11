# Emby → Parlour cutover checklist (E4)

Phase 1's finish line is operational, not code: **a week of real
household use with Emby off** — movies + TV, both users,
subtitles on, resume working, zero "we had to go back to Emby
for X" moments. This checklist is the run.

## Before the week

1. **Deploy Parlour on sullivan** (the production host, D4)
   beside Emby: `docker compose up -d` with the real `.env`
   (database password, admin bootstrap, `TMDB_API_KEY`,
   `PARLOUR_TMDB_API_KEY` if set separately,
   `PARLOUR_EMBY_URL` + `PARLOUR_EMBY_API_KEY` for the import,
   `PARLOUR_STREAM_SECRET` so stream tokens survive restarts,
   `PARLOUR_COOKIE_SECURE=true` behind Caddy).
2. **Libraries**: create the real movie/show libraries in Admin,
   point them at the media paths, grant each profile access,
   scan, then run metadata matching; clear the review queue.
3. **Accounts**: invite Kayla (Admin → Users & invites), create
   her profile, set PINs if wanted.
4. **Import watch state** (Admin → Emby import):
   - Dry run for Jordan's Emby user → Jordan's profile. Read the
     report: unmatched and ambiguous lists are the fix-list
     (usually missing TMDB ids in Parlour — match those items by
     hand in the review queue, then re-run the dry run).
   - Import for real. Repeat for Kayla's Emby user → her profile.
   - Spot-check: Parlour's Continue Watching should look like
     Emby's did.
5. **Clients**: point the TV/laptop browsers at Parlour
   (tailnet name on :8585); keep Emby installed but unused.

## The week

- Use Parlour for everything. Log every "had to go back to
  Emby" moment and every pain (however small) as an issue.
- Subtitles on, resume across devices (start on TV, finish on
  laptop) at least once each.

## After the week

- Fix the top pains (new milestone or Phase 2 pull-forwards).
- Port swap (D1): Caddy/public routing moves to Parlour as the
  household default; Emby's port closes.
- Emby stays installed, data untouched, for one more month as
  the rollback — then it's retired.

## Rollback

At any point before the port swap: stop using Parlour, keep
using Emby. Nothing in this process writes to Emby (the client
is read-only), and Parlour's imported state is a copy.
