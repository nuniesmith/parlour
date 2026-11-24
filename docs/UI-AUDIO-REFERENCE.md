# Audio UI reference — Audiobooth and Overcast (2026-11-24)

What the owner's two current audio apps look like, screen by
screen, and the Parlour audio UI derived from them for the web UI
and the future mobile app. The audiobook side follows Audiobooth,
the Audiobookshelf client he uses today; the podcast side follows
Overcast. Decisions it leans on: **D26** (Parlour's own native
clients, no ABS bridge), **D28–D35** (mirror Audiobookshelf where
it has an equivalent), all in `DECISIONS.md`. The phase plan is
`AUDIOBOOKS-PODCASTS.md`.

**Sources.** 25 iPhone screenshots the owner supplied on
2026-11-24, kept **outside the repo** in
`~/parlour-ui-reference/` on oryx (`audiobooth/`, `overcast/`).
The images are not copied here; each screen below names its file.

**Clean-room rule.** Audiobooth and Overcast are third-party apps
(Overcast is proprietary; Audiobookshelf, which Audiobooth talks
to, is GPL-3.0). As with Emby (D13), Parlour copies *layout
patterns and behaviour*, never their code, icons, artwork,
colours or wording. Labels quoted in §1–§2 are there to identify
the screens; Parlour's own labels are its own and follow
Parlour's tokens and branding.

---

## 0. Screenshot inventory

The owner described the set as 14 Audiobooth + 11 Overcast. The
folders hold 12 + 13, and two of the files in `audiobooth/`
(`IMG_0560`, `IMG_0561`) are Overcast's Settings screen (they
say "Overcast Premium", "Rate & Review Overcast"). By content the
set is **10 Audiobooth screens and 15 Overcast screens**. All 25
were reviewed.

| File | Folder | App shown | Screen | § |
|---|---|---|---|---|
| `IMG_0557.PNG` | audiobooth | Audiobooth | Player (full screen) | 1.3 |
| `IMG_0558.PNG` | audiobooth | Audiobooth | Player → Speed sheet | 1.4 |
| `IMG_0559.PNG` | audiobooth | Audiobooth | Home tab, mini-player, tab bar | 1.2 |
| `IMG_0560.PNG` | audiobooth | **Overcast** | Settings (top half) | 2.7 |
| `IMG_0561.PNG` | audiobooth | **Overcast** | Settings (bottom half) | 2.7 |
| `IMG_0565.PNG` | audiobooth | Audiobooth | Settings root | 1.5 |
| `IMG_0566.PNG` | audiobooth | Audiobooth | Settings → General | 1.6 |
| `IMG_0567.PNG` | audiobooth | Audiobooth | Settings → Home | 1.7 |
| `IMG_0568.PNG` | audiobooth | Audiobooth | Settings → Player (top) | 1.8 |
| `IMG_0569.PNG` | audiobooth | Audiobooth | Settings → Player (bottom) | 1.8 |
| `IMG_0570.PNG` | audiobooth | Audiobooth | Settings → Storage | 1.9 |
| `IMG_0571.PNG` | audiobooth | Audiobooth | Settings → Advanced | 1.10 |
| `IMG_0552.PNG` | overcast | Overcast | Now Playing | 2.3 |
| `IMG_0553.PNG` | overcast | Overcast | Now Playing → episode info + Chapters | 2.4 |
| `IMG_0554.PNG` | overcast | Overcast | Now Playing → Transcript | 2.5 |
| `IMG_0555.PNG` | overcast | Overcast | Now Playing → Audio sheet | 2.6 |
| `IMG_0556.PNG` | overcast | Overcast | Home (episodes, playlists, podcasts) | 2.2 |
| `IMG_0572.PNG` | overcast | Overcast | Settings → Listening Stats | 2.8 |
| `IMG_0573.PNG` | overcast | Overcast | Settings → History | 2.9 |
| `IMG_0574.PNG` | overcast | Overcast | Settings → Manage Storage | 2.10 |
| `IMG_0575.PNG` | overcast | Overcast | Settings → Download | 2.11 |
| `IMG_0576.PNG` | overcast | Overcast | Settings → Delete (played episodes) | 2.12 |
| `IMG_0577.PNG` | overcast | Overcast | Settings → Episode Limits | 2.13 |
| `IMG_0578.PNG` | overcast | Overcast | Settings → Audio Settings | 2.14 |
| `IMG_0579.PNG` | overcast | Overcast | Settings → Nitpicky Details | 2.15 |

**Not in the set** (so nothing below about them is from a
screenshot): Audiobooth's Library, Collections, Downloads and
Search tabs, its book detail page, chapter list, sleep-timer and
bookmark sheets, and the Up Next / Controls & Layout / Skip /
Sleep Timer / Playback Display / Lock Screen sub-pages; Overcast's
show page (a podcast's episode list), a playlist's contents, the
Up Next/queue view, search and add-podcast, the sleep-timer sheet,
and the Theme / Customize Home Screen / Promos & Reruns /
Notifications / Import & Export pages. §4 marks every proposal
that reaches into those areas as *not grounded in a screenshot*.

---

## 1. Audiobooth (audiobooks)

### 1.1 Navigation structure

- **Bottom tab bar** (`IMG_0559`): Home, Library, Collections,
  Downloads, plus a separate round **Search** button to the right
  of the bar. The active tab is tinted.
- **Library switcher** (`IMG_0559`): a pill at the top left with
  a green status dot and the current library's name
  ("Audiobooks"). Audiobookshelf servers have several libraries;
  this is where the client picks one.
- **Settings**: a gear button at the top right of Home opens
  Settings as a full-height **modal sheet** with a close (×)
  button (`IMG_0565`). Sub-pages push inside the sheet with a back
  chevron (`IMG_0566`–`IMG_0571`).
- **Mini-player** (`IMG_0559`): a floating capsule above the tab
  bar on every tab — cover thumbnail, "Title (Author)", time
  remaining, a round play button and a queue/list button. Tapping
  it opens the full player.
- **Full player** (`IMG_0557`): a full-screen sheet over
  everything, dismissed with a chevron-down at the top left.

### 1.2 Home (`IMG_0559`)

Top to bottom:

1. Library pill (left) and settings gear (right).
2. A large greeting with the server username ("Hi, jordan"); a
   setting can hide the name (§1.7).
3. **Continue Listening** — a horizontal row of covers. Each card:
   cover with a **percent-complete badge** in the top-left corner
   ("23%") and a small **downloaded** glyph in the bottom-right
   corner; below it the title (truncated) and **time remaining**
   ("8h 47min remaining").
4. **Recently Added** — horizontal row; card = cover, title,
   author(s) (truncated).
5. **Recent Series** — horizontal row; card = the series' cover
   with a **check mark** (series progress) top-left and a
   **book-count** badge ("1") top-right; series name below.
6. **Discover** — a further row, mostly hidden under the
   mini-player in the screenshot.

Shelves are ordered and toggled in Settings → Home (§1.7).

### 1.3 Player (`IMG_0557`)

- **Background** tinted from the cover's colours (purple here).
- **Top bar:** chevron-down (dismiss) left; an audio-route
  (AirPlay) button and a "…" menu right.
- **Cover** large and centred, with the **percent badge** ("23%")
  repeated at its top-left.
- **Chapter row:** a list icon and the current chapter's name
  ("02"); tapping it is the way into the chapter list.
- **Scrubber is chapter-scoped:** elapsed in the chapter on the
  left ("0:27:02"), remaining in the chapter on the right
  ("-1:09:11"), and the **whole book's remaining time** centred
  between them ("8h 47min remaining").
- **Transport row** (five buttons): previous chapter, back 30 s,
  play/pause (large), forward 30 s, next chapter.
- **Bottom bar** (a rounded capsule with three labelled buttons):
  **Speed** (shows the current rate, "1.5×"), **Timer** (sleep
  timer), **Bookmarks**. Which buttons sit here is configurable
  (Settings → Player → Controls & Layout: "Speed, Timer,
  Bookmarks +5", §1.8).

### 1.4 Speed sheet (`IMG_0558`)

A bottom sheet over the dimmed player: an **Edit** button
(top-left, edits the presets), the title, the current rate large
("1.50×"), a **slider with − / + buttons**, and a **2 × 3 grid of
presets** — 0.70, 1.00 (marked as the default), 1.20, 1.50
(selected, outlined), 1.70, 2.00.

### 1.5 Settings root (`IMG_0565`)

Large-title list in grouped cards, each row an icon tile, a title
and a one-line summary of what is inside:

- **Preferences:** General (launch, haptics, appearance) · Home
  (sections & order) · Player (controls, sleep, skip) · Storage
  (shows "928.8 MB · 2 books") · Advanced (system integrations).
- **Sponsor:** supporter status (the app's own tip jar).
- **Support:** help & feedback, Discord, email — external links.

### 1.6 General (`IMG_0566`)

- Behaviour: **haptic feedback** toggle.
- Appearance: **app icon** (three colours, each with light/dark
  variants), **theme** (three palettes: Sepia / System / Pure),
  **accent colour** (nine swatches + a custom picker),
  **colour scheme** (Light / Dark / Auto).

### 1.7 Home settings (`IMG_0567`)

- **Show username** in the Home title (toggle).
- **Continue shelf style:** a style picker ("Carousel"), a
  **cover size** choice (Default / Large / Extra Large) and
  **show time remaining** (toggle; when off, the author is shown
  under the cover instead).
- **Sections** list with drag handles and a "reset order" link:
  Continue Listening and Continue Reading are *always visible*
  (no toggle); Continue Series, Newest Episodes, Recently Added
  (and more below the fold) each have a visibility toggle.

### 1.8 Player settings (`IMG_0568`, `IMG_0569`)

- **Narration speed:** "default for new books" — the rate shown
  large (1.50×) with + / − buttons and the same six presets as
  the sheet. So speed is **per book**, with a default that new
  books start at. Two toggles: **time remaining scales with
  speed**, and **chapter times scale with speed**.
- **Behaviour** toggles: open the player on launch; an "assisted"
  player with only large play and skip buttons; keep the screen
  awake while the player is open; mix with other audio; pause at
  zero volume (and resume when raised); offer to catch up when an
  ebook has been read further than the audio has been heard.
- **Configure** (sub-pages, not in the set): Up Next ("queue and
  smart continue"), Controls & Layout (which buttons the player
  shows), Skip & Smart Rewind (**back 30 s · forward 30 s**),
  Sleep Timer & Alarm (a sensitivity setting, shown "Medium"),
  Playback Display ("Auto"), Lock Screen ("skip by seconds").

### 1.9 Storage (`IMG_0570`)

- A **donut chart** of device storage used (929 MB) with a legend
  split by kind: audiobooks (898.5 MB, 2 books), ebooks, podcasts
  (0 episodes), image cache, database.
- **Downloads:** **auto-download** while playing (Off / Wi-Fi only
  / Wi-Fi & cellular) with a delay; **keep next items offline**
  ("what's next in started series and podcasts", same three-way
  choice) with a count ("2 items"); auto-download queued episodes
  (toggle); **remove after completion** (toggle, on).

### 1.10 Advanced (`IMG_0571`)

**iCloud sync** of the app's preferences across the owner's
devices (on), and **NFC tag writing** (write a book to a tag for
quick playback; off).

---

## 2. Overcast (podcasts)

### 2.1 Navigation structure

- **No tab bar.** One scrolling **Home** screen (`IMG_0556`) with
  a gear (Settings) at the top left and a search button at the
  top right.
- **Settings** is a modal sheet with a close (×) (`IMG_0560`);
  sub-pages push with a back chevron and a title
  (`IMG_0572`–`IMG_0579`).
- **Mini-player** (`IMG_0556`): a capsule at the bottom — artwork,
  show name in small caps, episode title — with the **pause
  button as a separate round button** beside the capsule.
- **Now Playing** (`IMG_0552`) is a full-screen view; **scrolling
  down** inside it reveals episode info, chapters and the
  transcript (`IMG_0553`, `IMG_0554`) while a compact control dock
  stays pinned at the bottom.

### 2.2 Home (`IMG_0556`)

1. **Episode cards:** a two-row, horizontally scrolling grid of
   recent episodes. Each card: show artwork, a status word in
   small caps (**PLAYING** for the current one, **NEW** for
   unplayed new ones), the episode title (two lines).
2. **Playlists:** a horizontal row of round icon buttons with
   labels — All Episodes (selected), Downloaded, In Progress,
   Starred, more off-screen. These are filtered views, not only
   hand-made lists.
3. **Podcasts:** a section header with a **Current / All**
   segmented control (shows with unplayed episodes vs every
   subscription). Rows: square artwork, **show title** (up to two
   lines), **author/network in small caps**, a chevron into the
   show page (not in the set).

### 2.3 Now Playing (`IMG_0552`)

- **Header:** the show name in small caps above the episode title
  (no cover-colour tint; dark grey background).
- **Artwork** large, square.
- **Scrubber is episode-scoped,** with the **current chapter's
  title** above it ("THREADS ARE FOREVER") and two round buttons
  flanking it: **chapter list** (left) and **next chapter**
  (right). Elapsed (23:24) and remaining (−1:00:56) for the whole
  episode below.
- **Transport:** back **15 s**, play/pause (Overcast's pause glyph
  shows playback progress inside the bars), forward **30 s**.
  There are **no previous/next-episode buttons** on this screen.
- **Bottom row:** chevron-down (dismiss) left; a centre pill with
  three icons — **audio effects** (§2.6), **audio route**,
  **sleep timer**; "…" menu right.

### 2.4 Episode info and chapters (`IMG_0553`)

Scrolling the player down: the show name as a link, the episode
title, the publish date, then **Chapters** — each row a **check
mark** once played, the chapter title, its length; a sponsor
chapter carries a link out; the **current chapter is
highlighted** with a fill showing how far through it playback is
and its remaining time ("−2:30"). A compact dock (scrubber with
the chapter and next-chapter buttons, 15 / pause / 30, and the
bottom row) stays pinned.

### 2.5 Transcript (`IMG_0554`)

A "Transcript" view (from the episode's own transcript):
timestamped paragraphs, the **current sentence highlighted**, a
button to toggle speaker/caption display and an **auto-scroll /
follow playback** toggle; the same pinned dock.

### 2.6 Audio sheet (`IMG_0555`)

A bottom sheet from the effects button: a preset chip ("Normal"),
**playback speed** (1.5×, − / +), **Smart Speed** (shortens
silences), **Voice Boost** (levels volume), premium effects (mono,
EQ, noise reduction), and **"custom for this podcast"** — a
toggle that makes these settings a per-show override instead of
the global ones.

### 2.7 Settings root (`IMG_0560`, `IMG_0561`)

Grouped cards with coloured icon tiles; rows show their current
value on the right:

- Sync profile (account) · Notifications.
- Premium · **Listening Stats** · **History & Undo**.
- **Manage Storage** ("1.1 GB") · **Download** ("Automatically") ·
  **Delete** ("When Completed") · **Episode Limits** · Promos &
  Reruns.
- **Audio Settings** · Seek back by (**15 s**) · Seek forward by
  (**30 s**) · seek with the lock-screen slider (toggle).
- Theme · Customize Home Screen · Nitpicky Details.
- Feedback / review / beta links · **Import & Export** · About.

### 2.8 Listening Stats (`IMG_0572`)

A segmented **List / Image to Share** control; a **year** with
previous/next arrows; a sort menu ("Listening Time"); rows of
**show artwork, show name, total listening time** (h:mm:ss),
longest first; a floating **Year / Month / Day** segmented control
at the bottom.

### 2.9 History (`IMG_0573`)

Grouped by time ("Last hour", "Today", …). Each row: show
artwork, when (relative or weekday + time), the episode title,
and **what happened** — "listened from 2:08:10 to 2:20:15", with
"(END)" when it reached the end — or "deleted after completion"
with a trash icon. One listen can appear as several rows (one per
session). The settings row calls it "History & Undo": it is the
place to reverse an accidental delete or mark.

### 2.10 Manage Storage (`IMG_0574`)

Totals for downloaded episodes, images and podcast/episode data,
then **per-show storage** largest first.

### 2.11 Download settings (`IMG_0575`)

Use cellular data (toggle); **download new episodes:
Automatically / Manually**; then **"customize by podcast"** — a
row per show with a picker to override the global choice.

### 2.12 Delete played episodes (`IMG_0576`)

**Manually / When Completed / 24 Hours After Completion**; a
**Smart Complete** toggle (an episode with under 5 minutes left
and untouched for 48 hours counts as complete); then per-show
overrides as in §2.11.

### 2.13 Episode Limits (`IMG_0577`)

A row per show with a **limit picker** (all "Unlimited" here) and
a **Set All** button.

### 2.14 Audio Settings (`IMG_0578`)

The global version of §2.6: speed, Smart Speed, Voice Boost,
premium effects, with the note that shows with custom settings
are excluded.

### 2.15 Nitpicky Details (`IMG_0579`)

Toggles, each with an explanation: stop after each episode;
**smart resume** (rewind a little after a pause, land seeks
between words); seek acceleration (repeated seeks skip further);
sleep-timer fade-out; one-tap play (play from a list instead of
expanding the row); play the top unplayed episode of a playlist
next instead of the following one; more below the fold.

---

## 3. The two side by side

| Aspect | Audiobooth (books) | Overcast (podcasts) |
|---|---|---|
| Top-level navigation | Bottom tab bar (Home, Library, Collections, Downloads) + Search; library switcher pill | One Home screen; Settings and Search as corner buttons |
| Home | Shelves: Continue Listening, Recently Added, Recent Series, Discover (reorderable) | Episode cards (Playing/New), Playlists row, Podcasts list (Current/All) |
| List row / card | Cover with % badge and downloaded glyph; title; time remaining or author | Show: artwork, title, author in small caps. Episode card: artwork, status word, title |
| Mini-player | Capsule: cover, "Title (Author)", time remaining, play, queue | Capsule: art, show name, episode title; separate round pause |
| Player scrubber | **Chapter**-scoped, book remaining in the middle | **Episode**-scoped, chapter title above |
| Chapter navigation | Prev / next chapter in the transport row; chapter name opens the list | Chapter list + next chapter flanking the scrubber; chapters inline below the player |
| Skip | Back 30 s / forward 30 s | Back 15 s / forward 30 s |
| Bottom controls | Speed · Timer · Bookmarks (configurable) | Audio effects · route · sleep timer; "…" |
| Speed | Per book, with a default for new books; presets 0.7–2.0 | Global, with a per-show override; − / + |
| Extra audio | — | Smart Speed, Voice Boost, EQ etc. (on device) |
| Extra views | — (bookmarks in the player) | Transcript, listening stats, history |
| Offline | Auto-download while playing; keep next N offline; remove after completion | Download new automatically / manually per show; delete when completed / 24 h after; per-show limits |
| Settings sheet | Large title, grouped cards with summaries | Grouped cards with current values |

---

## 4. Proposed Parlour audio UI

A proposal for the owner to amend, not a decision. Audiobooks
follow Audiobooth (§1), podcasts follow Overcast (§2); what the
screens *show* and what *syncs* follows the 2026-11-24 decisions.
Items marked **[no screenshot]** reach into screens that aren't in
the set (§0) and are inferred from the references' behaviour plus
the decisions.

### 4.1 Ground rules from the decisions

- **Parlour's own clients** (D26): the web UI now, Parlour's mobile
  app later. No ABS client has to keep working, so the layout is
  free to follow the references.
- **Books and podcasts in one app** (D34, as ABS's own app does).
  Whether that app is also the video app is *no ABS equivalent —
  deferred until Jordan asks*. The proposal is written as an
  **audio area** that works either way: inside the video app it is
  what an audio library opens into; as a separate app it is the
  whole app.
- **Library-centred, as ABS is** (D31): podcasts are a library like
  audiobooks, so both apps' structures hang off "which library am I
  in" — Audiobooth's library pill (§1.1) is the switch.
- **What the screens show mirrors ABS** (D28, D29, D31, D32): the
  Home shelves, series / author / narrator browsing, the
  finished rule, admin-managed shows, server downloads.
- **Three homes for settings** (D35): *profile* settings stored on
  the server and synced (Home shelf order, hidden items), *device*
  settings kept by the app or browser (speed, skip intervals, sleep
  timer, downloads, appearance), and *admin* settings per library or
  per show (podcast download and retention, metadata precedence).
  §4.6 places every setting seen in the screenshots.
- **Parlour branding.** Parlour's tokens, icons and words; no
  reference colours, glyphs (e.g. Overcast's progress-filled pause
  bars) or labels.

### 4.2 Navigation

**Mobile app**

- **Library pill** at the top left (from §1.1/§1.2): lists the
  profile's audio libraries, audiobooks and podcasts alike. The
  rest of the app re-forms around the selected library. A gear at
  the top right opens Settings as a modal sheet (both references
  do this, §1.5/§2.7).
- **Bottom tab bar** (from §1.1) for both kinds of library, so there
  is one muscle memory:

  | Tab | Audiobooks library | Podcasts library |
  |---|---|---|
  | Home | ABS's book shelves (§4.3) | Overcast's Home, whole (§4.4) |
  | Library | All books; Series, Authors, Narrators views (D28) [no screenshot] | All shows A–Z [no screenshot] |
  | Collections | Collections (shared) + the profile's playlists (D35) [no screenshot] | Playlists (D35) [no screenshot] |
  | Downloads | What's on this device (D33) [no screenshot] | Same |
  | Search (round button) | Books, authors, series, narrators [no screenshot] | Shows and episodes; admins also search Apple's directory to add a show (D31) [no screenshot] |

  Overcast has no tab bar (§2.1) — see conflict C1 in §4.5.
- **Mini-player** floating above the tab bar on every tab (both
  references), opening the full player as a sheet with a
  chevron-down to dismiss.

**Web UI**

- Audio libraries sit in the existing sidebar (D13 shell) like any
  library. An audio library's page carries the same sections as the
  mobile tabs as **tabs along the top of the page** (Home, Library,
  Series, Authors, Narrators, Collections for books; Home, Shows,
  Playlists for podcasts). The sidebar replaces the tab bar; there
  is no Downloads tab on the web (offline is the mobile app's,
  D33).
- **A persistent player bar** docked along the bottom of the shell,
  the web form of the mini-player, which keeps playing while the
  listener browses. It expands into the full player (a full-page
  view on phone widths, a right-hand panel on desktop). **This is a
  change from AB5**, where the `<audio>` element lives inside
  `/book/[id]` and stops when you leave the page. Moving playback
  into the root layout is a prerequisite web task.
- Keyboard: Space play/pause, ← / → skip back/forward, Shift + ← / →
  previous/next chapter, listed under Web Settings → Keyboard with
  the existing `/` shortcut.
- Pages: `/book/[id]` (exists) becomes the book page and stops being
  the only place playback lives; new `/podcast/[id]` (show page) and
  `/episode/[id]` (episode page, for deep links and show notes)
  [no screenshot].

### 4.3 Audiobooks (Audiobooth model)

**Home** (from §1.2, shelves per D29):

- An optional greeting with the **profile's** name (the profile is
  the listener, D29), not the login name. On/off per profile.
- Shelves, in ABS's set: **Continue Listening**, **Continue
  Series**, **Recently Added**, **Recent Series**, **Discover**,
  **Listen Again**, **Newest Authors**. Order and visibility per
  profile, stored on the server: the existing Settings → Home Screen
  page (UI7) gains the audio shelves, with Audiobooth's rule that
  Continue Listening can't be hidden (§1.7).
- **Continue Listening card:** cover with a **percent badge**, a
  **downloaded** glyph on mobile, title, then **time remaining** (or
  the author, a device setting, §1.7). A long-press / "…" offers
  **Hide from Continue Listening** (D29; the hide clears when
  progress next changes) and **Mark as finished**.
- **Series card:** the series' cover with a book count and a
  finished check when every book is done (from §1.2).
- Cover size (Default / Large / Extra Large) and the shelf style are
  device settings.

**Library** [no screenshot]: a cover grid with sort (title,
author, added, recently listened; AB3 already sorts by title,
added and recent) and filters (series, author, narrator, genre,
progress: not started / in progress / finished). Series, Authors
and Narrators views list those records (D28: series with books in
sequence order; a book in two series appears in both).

**Book page** [no screenshot]: cover, title, author(s), narrator(s),
series with sequence, year, duration, this profile's progress and
time remaining, **Play / Resume**, Mark finished / not finished
(a manual un-finish goes back to 0, D29), Download (mobile),
**bookmarks** list, **chapters** list (tap to play from there),
description, genres. Admins also see tracks and a Match action
(AB6, D30).

**Player** (from §1.3):

- Cover-tinted background, percent badge on the cover.
- Chapter row with the current chapter's title; tapping it opens
  the chapter list.
- **Chapter-scoped scrubber**: elapsed / remaining in the chapter
  at the ends, **book time remaining** in the middle. A device
  setting switches the scrubber to the whole book.
- Transport: **previous chapter · back 30 s · play/pause · forward
  30 s · next chapter**.
- Bottom bar: **Speed** (showing the rate), **Sleep timer**,
  **Bookmarks**. Mobile adds the audio-route button and a "…" menu
  (go to book, chapters, mark finished, download).
- **Speed sheet** (from §1.4): rate, slider with − / +, a preset
  grid (0.7, 1.0, 1.2, 1.5, 1.7, 2.0, editable). Speed is kept
  **per book on the device**, with a default for new books (from
  §1.8). It is not synced (D35).
- **Bookmarks sheet** [no screenshot]: add at the current time with
  an optional title; list, jump, rename, delete. **Synced** per
  profile per book (D35).
- **Sleep timer sheet** [no screenshot]: durations plus "end of
  chapter"; on the device only (D35).
- Time displays can scale with speed (device setting, from §1.8).

**Downloads and storage** (mobile, from §1.9; D33): a download
button on the book page and the "…" menu; the Downloads tab lists
what's on the device; Storage shows a breakdown by kind (books,
podcasts, images, data) and per-item sizes (Overcast's per-show
list, §2.10); options: auto-download while playing (off / Wi-Fi /
Wi-Fi + cellular), keep the next *N* items of started series
offline, remove after finished. All device-side.

### 4.4 Podcasts (Overcast model)

**Home** (from §2.2):

1. **Episode cards**, two rows scrolling sideways: the playing
   episode first, then the newest episodes this profile hasn't
   finished across the library's shows (ABS's "Newest Episodes"
   shelf, D29/D31). Each card: show art, a status word (playing /
   new / in progress), the episode title.
2. **Playlists row** of round buttons: built-in views — **All
   episodes**, **Downloaded** (on this device), **In progress** —
   then the profile's own playlists (synced, D35). Overcast's
   **Starred** has *no ABS equivalent — deferred until Jordan
   asks*.
3. **Shows list** with a **Current / All** switch: artwork, title
   (two lines), author in small caps, chevron. "Current" is
   computed from the profile's own progress: shows with episodes it
   hasn't finished. Under D31 every profile sees every show in the
   library, so a show one profile never listens to stays "current"
   for it. A per-profile follow / hide list would fix that; it has
   *no ABS equivalent — deferred until Jordan asks*.

**Show page** [no screenshot]: artwork, title, author, description;
the episode list with each row's title, date, duration and this
profile's state (new, part-played with a progress ring, finished
check); episodes the server hasn't downloaded are marked and, for
admins, offer **Download** (only downloaded episodes play, D32).
Tapping a row expands it (play, add to playlist, mark finished,
download to device); a device setting plays on one tap instead
(from §2.15). **Admins** get the show's settings here: auto-download
(off), check schedule (hourly), keep *N* (0 = all), new per check
(3), refresh now (D32 defaults).

**Player** (from §2.3–§2.5):

- Show name in small caps over the episode title; square artwork.
- **Episode-scoped scrubber** with the **current chapter's title**
  above it, **chapter list** and **next chapter** buttons flanking
  it, elapsed / remaining for the episode.
- Transport: **back 15 s · play/pause · forward 30 s**. No
  previous / next episode buttons (Overcast has none on this
  screen; the playlist decides what's next).
- Bottom row: dismiss, then **speed** (in the place of Overcast's
  effects button), audio route (mobile), **sleep timer**, "…".
- **Scroll down** for episode info (show link, date), **show notes**
  and **chapters** with a played check per chapter and the current
  one filled to its position (from §2.4), with a compact control
  dock pinned at the bottom.
- **Transcript view** (§2.5): *no ABS equivalent — deferred until
  Jordan asks.*
- **Audio sheet** (§2.6) is speed only, with "custom for this show"
  (Overcast's per-show override, kept on the device, D35). **Smart
  Speed, Voice Boost and the other effects:** *no ABS equivalent —
  deferred until Jordan asks.*

**History and stats** (from §2.8/§2.9; D29/D35 — ABS keeps
listening sessions and stats on the server):

- **History**: this profile's listening sessions newest first,
  grouped by day, each row "*episode or book* — listened from
  *a* to *b*" (with an end marker). Overcast's undo of deletes and
  marks has *no ABS equivalent — deferred until Jordan asks.*
- **Listening stats**: total time per show and per book, by year /
  month / day, from the same sessions (CL3). The shareable image
  view: *no ABS equivalent — deferred.*

Both pages cover books as well as podcasts (sessions are one
model), reached from Settings on mobile and from the profile menu
on the web.

### 4.5 Where the references conflict, and the proposal

| # | Conflict | Audiobooth | Overcast | Proposal | Why |
|---|---|---|---|---|---|
| C1 | Top navigation | Tab bar + library pill | One Home, no tabs | Tab bar for both; the podcast Home tab holds Overcast's whole Home | One app (D34); podcasts are a library (D31); an Overcast-style user never has to leave Home |
| C2 | Mini-player | Play inside the capsule; time remaining; queue button | Pause as a separate circle; show name + title | One capsule: art, title, second line (book: time remaining, episode: show name), play/pause inside, queue button | One component; the queue is on the device (D35) |
| C3 | Scrubber scope | Chapter, with book remaining | Episode, with chapter title | Per kind as each reference does; device setting to switch | Books run for hours; episodes are about one |
| C4 | Chapter controls | Prev/next chapter in the transport row | List + next chapter beside the scrubber | Per kind as each reference does | Keeps each reference's layout for its kind |
| C5 | Skip intervals | Back 30 / fwd 30 | Back 15 / fwd 30 | Device setting per kind; defaults 30/30 books, 15/30 podcasts | Device-only in ABS (D35) |
| C6 | Speed scope | Per book, default for new books | Global, per-show override | Per kind as each reference does, all on the device | ABS keeps no speed on the server (D35) |
| C7 | Bottom controls | Speed · Timer · Bookmarks | Effects · route · timer | Books: speed · timer · bookmarks. Podcasts: speed · route · timer | ABS has no episode bookmarks (D35); effects deferred |
| C8 | After finishing | Remove download after completion | Delete when completed / after 24 h; Smart Complete | Device: remove download after finished, now or after 24 h. Server: ABS's keep-*N* per show (admin). Finished = ABS's under-10-s rule (D29) | Two layers: what the phone keeps vs what the server keeps (D32/D33); Smart Complete has no ABS equivalent — deferred |
| C9 | Episode limits | — | Per show, on the phone | Admin per show on the server (keep *N*, D32) | Storage is the server's, shared by every profile |
| C10 | New-episode download | — | Auto / manual per show, on the phone | Server: admin auto-download per show (D32). Phone: auto-download new episodes to the device, per show (D33) | Same two layers as C8 |
| C11 | Extra views | Bookmarks | Transcript, stats, history | Bookmarks (books), stats + history (both kinds); transcript deferred | ABS has bookmarks and sessions; no transcripts |
| C12 | Settings | Large-title sheet with summaries | Grouped cards with values | Grouped cards with each row's summary **or** current value, in Parlour's style | Both are grouped cards; values help most |
| C13 | Appearance | Theme, accent, app icon | Theme | Light / dark / auto per device only; icons and accent colours: no ABS equivalent — deferred | Parlour branding |

### 4.6 Where each setting lives

| Setting (seen in) | Home | Decision |
|---|---|---|
| Home shelf order / visibility (`IMG_0567`) | Profile, synced | D29 shelves; UI7 Home Screen page |
| Hide from Continue Listening | Profile, synced | D29 |
| Bookmarks (`IMG_0557`) | Profile, synced (books only) | D35 |
| Playlists; collections | Profile, synced; collections shared per library | D35 |
| Progress, finished | Profile, synced | D29 |
| Auto-finish threshold (10 s / %) | Admin, per library | D29 |
| Podcast auto-download, schedule, keep *N*, new per check | Admin, per show | D32 |
| Metadata precedence, match | Admin, per library | D30 |
| Speed and presets (`IMG_0558`, `IMG_0568`, `IMG_0555`, `IMG_0578`) | Device | D35 |
| Skip back / forward (`IMG_0569`, `IMG_0561`) | Device | D35 |
| Sleep timer, fade-out (`IMG_0569`, `IMG_0579`) | Device | D35 |
| Up Next queue (`IMG_0569`) | Device | D35 |
| Downloads: auto, keep next *N*, remove after finished, cellular (`IMG_0570`, `IMG_0575`, `IMG_0576`) | Device | D33 |
| Time scales with speed; scrubber scope; one-tap play; open player on launch; keep screen awake (`IMG_0568`, `IMG_0579`) | Device | Client behaviour; no server part |
| Theme / colour scheme (`IMG_0566`) | Device | Parlour branding |
| Smart Speed, Voice Boost, effects (`IMG_0555`, `IMG_0578`) | — | No ABS equivalent — deferred until Jordan asks |
| Smart Resume / smart rewind, seek acceleration, pause at zero volume, mix with other audio, assisted player, lock-screen seek (`IMG_0568`, `IMG_0569`, `IMG_0579`, `IMG_0561`) | — | Client conveniences, no server part; which ones the app gets: deferred until Jordan asks |
| Smart Complete, Promos & Reruns, Starred, undo (`IMG_0576`, `IMG_0560`, `IMG_0556`, `IMG_0573`) | — | No ABS equivalent — deferred until Jordan asks |
| iCloud preference sync, NFC tags, app icons, accent colour (`IMG_0571`, `IMG_0566`) | — | No ABS equivalent — deferred until Jordan asks |
| Continue Reading, catch-up with the ebook (`IMG_0567`, `IMG_0568`) | — | Parlour has no ebook library (ROADMAP "Growth after that") |
| OPML import / export (`IMG_0561`) | Admin | D31 |

---

## 5. Decision trace

| Proposal | Decision |
|---|---|
| Parlour's own web + mobile audio UI; no ABS client kept working | D26 |
| No "import from Audiobookshelf" screen | D27 |
| Library / Series / Authors / Narrators views; a book in several series; editions are separate books | D28 |
| Per-profile progress; ABS's shelves; hide from Continue Listening; auto-finish; history and stats from sessions; no play count | D29 |
| Admin Match on the book page; embedded covers | D30 |
| Podcasts as a library in the pill; admin-only add via Apple search / URL / OPML; no subscribe button | D31 |
| Only downloaded episodes play; admin per-show download / retention | D32 |
| Mobile Downloads tab and storage page; web has none | D33 |
| Books + podcasts in one app; app-vs-video shape deferred | D34 |
| Synced vs device settings (§4.6); queue on the device; effects deferred | D35 |

## 6. Deferred — no ABS equivalent (until Jordan asks)

Smart Speed, Voice Boost and other audio effects · transcripts ·
Starred episodes · a per-profile follow / hide list for shows ·
Smart Complete · Promos & Reruns · History undo · the shareable
stats image · iCloud-style preference sync · NFC tags · app icon
and accent choices · the smaller client conveniences in §4.6 · a
separate audio app vs one app with video · audio on the Fire Stick
· a tablet-specific layout · iOS before Android for audio.

## 7. Screens worth capturing next

If the owner wants §4's **[no screenshot]** parts grounded:
Audiobooth's Library tab, Collections tab, Downloads tab, book page,
chapter list, sleep-timer and bookmark sheets, and its Up Next
settings; Overcast's show page, an expanded episode row, a playlist,
and the Up Next / queue view.
