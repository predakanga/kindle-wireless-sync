# 0026: Read sync only on firmware 5.19.1 and later

- Status: Proposed
- Date: 2026-10-20
- Amends: [0017](0017-read-status-sync.md) (when read sync runs) and
  [0020](0020-one-screen-pages-and-checking-a-library.md) (the read-sync
  settings in the app)

## Context
Read sync (ADR 0017) reads three things from `cc.db`'s `Entries` for each
book under the books folder: `p_location` (with `p_type`), `p_readState`
and `p_lastOpenTime`, in one query (`readsync::kindle_states`).

**Which firmware has those columns** (research note, "`p_lastOpenTime`
arrived in 5.19.1", 2026-10-20). The catalog's schema migrations are plain
SQL in each firmware's root filesystem, and compat runs list missing
columns:
- `p_location` and `p_type` are in the base `Entries` table.
- `p_readState` is migration 75. It's in every firmware we've looked at:
  5.16.2.1.1 (Basic 2016, compat run), 5.17.1.0.2 and 5.18.6 (Paperwhite
  6, rootfs).
- **`p_lastOpenTime` is migration 81, first in 5.19.1** (Scribe 3 and
  Scribe Colorsoft only). The other models we checked went from 5.18.6
  straight to 5.19.2.

On older firmware SQLite rejects the whole query ("no such column"), so
neither value reaches Calibre, and the daemon logs a warning for every
session. The user sees "Reading status sync is on" in the app, but nothing
happens.

**`p_readState` existing doesn't show the firmware keeps it.** The 5.18.1
report the research note cites found the "Mark as Read" state in
`fmcache.db` and the cloud, not in `cc.db`. The catalog's own code that
writes `p_readState` (`UPDATE Entries SET p_readState = ?`) is already in
5.17.1.0.2, but whether the Library calls it there is unproven. The
values ADR 0017 relies on (0, 1, 2, 3) were only probed on 5.20.1.

**The firmware version** is in `/etc/prettyversion.txt` as `Kindle
5.20.1 (…)` on the reference device and in every Paperwhite 6 image from
5.17.1.0.2 to 5.20.1. KOReader's `platform/kindle/koreader.sh` reads the
version from the same line (`grep '^Kindle 5' /etc/prettyversion.txt`).

The developer asked for read sync to be off on firmware older than
5.19.1 (2026-10-20).

## Decision
**Read sync runs only when the firmware is 5.19.1 or later.** On older
firmware it's unavailable, whatever `config.json` says.

**The firmware version** comes from the first line of
`/etc/prettyversion.txt` matching `Kindle X.Y.Z…`, compared number by
number (`5.17.1.0.2` < `5.19.1` < `5.19.2`). It's read once when the
daemon starts: a firmware update reboots the Kindle. If there's no such
file or line, as when the daemon runs on the Mac, read sync is allowed,
as today. Only a version that is read and is older turns it off.

**The daemon:** the session names no sync columns in its handshake, so
Calibre doesn't ask for `_is_read_` or `_last_read_date_`, and
`Library::read_sync_fields` doesn't query the catalog. It logs once, at
start, that read sync is unavailable on this firmware.

**The settings are kept.** `writable` and the chosen columns stay in
`config.json`, so they apply if the Kindle's firmware is updated later. A
link made on old firmware starts with read sync off, as every new link
does (ADR 0018).

**The API** reports it: `/status` gains `read_sync_available` (bool) and
`firmware` (the version string, or `null`). `POST /link` and `POST
/settings` refuse `writable: true` on older firmware, with a message
naming the version needed.

**The app** shows, where the "Update read status in Calibre" switch would
be (the setup and settings pages, ADR 0020), a note instead: "Reading
status sync needs Kindle firmware 5.19.1 or later. This Kindle has
5.18.6." The first page's summary says "Reading status sync isn't
available on this Kindle". The note must fit the one-screen pages
(`ui_pages_fit`).

**The command line:** `link --writable` fails with the same message, and
`status` names the firmware and whether read sync is available.

**The compat check** (`tools/compat`) for the catalog schema reports a
missing `p_lastOpenTime` as expected below 5.19.1, rather than as a
failure.

## Alternatives considered
- **Check the catalog's columns instead of the version** (`PRAGMA
  table_info(Entries)`): exact about the schema, and needs no version
  parsing. But it would turn read sync on wherever `p_lastOpenTime`
  exists, which says nothing about whether the firmware keeps
  `p_readState` in `cc.db`. The version also explains itself to the user;
  a missing column doesn't.
- **Sync read status alone on older firmware** (leave out
  `p_lastOpenTime`): the date is the less important half. But the
  read state in `cc.db` is the unproven part before 5.19 (the 5.18.1
  report), and a half-working feature is harder to explain.
- **Leave it as it is:** the user sees it switched on and nothing syncs,
  with only a log line to say why.
- **Clear `writable` in `config.json` on old firmware:** loses the user's
  choice if they update the firmware later, for no gain.

## Consequences
- Users on firmware older than 5.19.1 can't sync reading status, and the
  app tells them why.
- One more place the firmware version matters. If other differences turn
  up (ADR 0001 makes other firmware best-effort), the same version reading
  serves them.
- Tests cover the version parsing and comparison, and the daemon, API and
  CLI behaviour with a given version; the daemon takes the version as an
  input rather than reading the file in tests.
- **To verify:** read sync on 5.19.x, where `p_readState` and
  `p_lastOpenTime` exist but the read states have only been probed on
  5.20.1 (`just compat` on 5.19.2 with a book marked read, or a probe as in
  the research note). If 5.19 doesn't keep `p_readState`, raise the
  threshold. Also the app's note on a 5.18.6 emulator instance, and that
  `/etc/prettyversion.txt` has the same form on other models.
