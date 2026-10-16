# 0017: Send read status and last-read date from the Kindle to Calibre

- Status: Accepted
- Date: 2026-10-16

## Context
Phase 5, stretch goal 2, is to send reading progress and ratings from the
Kindle back to Calibre. Two limits apply. Calibre and the daemon talk over
one connection only, the wireless one (developer, 2026-10-16). And a
Calibre plugin is deferred to the Later list for now. So the question is
what can reach Calibre through the stock wireless driver alone.

**Calibre itself has no read flag or last-read date.** A stock library has
neither field. What the wireless driver offers is an optional hook for two
custom columns that the user creates (`smart_device_app/driver.py`):
- In its handshake reply (`GET_INITIALIZATION_INFO`), the device can name
  one Yes/No column (`isReadSyncCol`) and one Date column
  (`isReadDateSyncCol`). It names them by lookup name, such as `#read`.
- The device then adds two fields to each book it lists (`GET_BOOK_COUNT`):
  `_is_read_` and `_last_read_date_`.
- Calibre checks once per connection that the named columns exist with the
  right types. If one doesn't, it shows the device an error message, and
  that connection syncs nothing.
- For each book, Calibre compares the device's value with the value it last
  sent the device, which travels in the book's metadata. If they differ,
  Calibre takes it as a change on the device and writes it into the column.
  If they match, the column keeps Calibre's value.
- A book flagged `_sync_type_: 2` is synced for the first time: the
  device's value wins if it has one, otherwise Calibre's value is kept.

That's all the stock driver can carry back. **Percent read** needs a
plugin. Calibre keeps progress in `last_read_positions`, which only its own
viewer and Content server write. The Content server takes updates over
HTTP, but that would be a second connection. The plugin options are in
`research/calibre-wireless-protocol.md`. **Ratings** are out of scope (see
Consequences).

**What the Kindle stores**, probed on 5.20.1 for a sideloaded book
(`research/kindle-library-integration.md`, "Read state probe"):
- `cc.db` `p_readState`: 0 = never set, 1 = marked read, 2 = marked read
  automatically at the end of the book, 3 = marked unread.
- `p_lastOpenTime`: set when the book is opened and when it's closed. It's
  unset until the first open. (`p_lastAccess` also changes when the file is
  indexed, so it doesn't mean the book was read.)

**Naming the columns.** There's no common convention for these columns'
names: Calibre Companion and the Kobo plugins all let the user choose.
Calibre sends the device the library's full list of columns and their types
in `SET_LIBRARY_INFO`, on every connection. It arrives after the handshake
in which the device names its columns.

## Decision
**Sync read status and last-read date, from the Kindle to Calibre only.** A
book marked read or unread on the Kindle, and the time it was last read
there, reach Calibre on the next connection. Changes made in Calibre stay
in Calibre: we don't change the Kindle's read state (developer, 2026-10-16;
the other direction is on the Later list).

**Only for libraries marked writable.** Each allowed library's entry in
`config.json` (ADR 0003) gets a boolean `"writable"`.
- A library with `writable: false` only sends books to the Kindle. We never
  name the columns for it, so nothing the Kindle does changes that library.
- A library with `writable: true` also takes changes from the Kindle: read
  status and last-read date now, and whatever a later plugin adds.
- **The default is `false`**, including for existing entries. Letting a
  Kindle write into a Calibre library should be a deliberate choice, made
  per library. For example, a shared household library might send books
  without being changed by this Kindle.
- A boolean, rather than a mode string, leaves no spellings to get wrong
  (developer, 2026-10-16).

**Into two columns the user creates in Calibre.** Their lookup names are
stored per library in the same entry: `"read_column"` (a Yes/No column) and
`"last_read_column"` (a Date column). The daemon uses whatever names are
stored, so it's ready for user-chosen columns. **For now, every caller
stores fixed names:** **`#kindle_read`** and **`#kindle_last_read`**
(developer, 2026-10-16). The headings can be anything. The `kindle_` prefix
keeps them apart from a user's own `#read`, which may have another type or
belong to another tool.
- For now, these settings are only set from the command line, with
  `allow --writable`, and with a way to change an already allowed library.
  Both store the fixed names.
- With the UI redesign (Later list), the app gets a toggle for `writable`,
  and can then let the user pick the columns from the library's own list
  of columns.

**Name a column only once it exists.** Naming a missing column would make
Calibre show an error on the Kindle at every connection, to every user who
hasn't created it. So for a writable library, we name a column only after
seeing it, with the right type, in that library's column list. We record
the columns we've seen per library, keyed by the library's UUID, and
refresh the record on every connection.
- The connection on which the columns first appear can't sync yet, because
  the handshake is already over. The daemon reconnects once, so the next
  connection syncs. It waits until Calibre is idle (see Implementation
  notes).
- On that first sync we flag each book `_sync_type_: 2`, so neither side's
  existing values are wiped.
- If a column disappears, we stop naming it from the next connection.

**What we report for each book:**
- `_is_read_`: true for `p_readState` 1 or 2, false for 3.
- `_last_read_date_`: `p_lastOpenTime`, as an ISO date.
- **Unless the Kindle hasn't changed.** Calibre treats any value that
  differs from what it last sent as a change made on the device. So if we
  just reported the Kindle's own value, it would overwrite every edit made
  in Calibre. We store, per book in `books.json`, the values Calibre last
  sent (from the book's `user_metadata`) and the Kindle values we last
  reported. If the Kindle's value has changed since then, we report it.
  Otherwise, or if the Kindle has no value (`p_readState` 0, or never
  opened), we report Calibre's value back unchanged.
- So the two sides can differ: a book marked read in Calibre stays unread
  on the Kindle until it's marked there. That's accepted for now.

The values are read from `cc.db` with read-only `sqlite3` queries (ADR
0005), at the start of each session, when we build the book list.

## Alternatives considered
- **Pick the columns from a list in our app:** preferred long-term, but
  waits for the UI redesign. The daemon already takes any names; only the
  callers use fixed ones.
- **Hard-code the fixed names in the daemon:** rejected. Storing the names
  per library costs little now and avoids a migration when users can pick
  them.
- **A mode string (`"access": "read-only"` / `"read-write"`):** replaced by
  the `writable` boolean (developer, 2026-10-16), which has no spellings to
  get wrong.
- **Type the column names into our app:** typing on the e-ink keyboard, and
  easy to get wrong.
- **Guess the columns by name** (say, any Yes/No column called "read"):
  rejected. A wrong guess writes into a column that means something else.
- **Always name the columns, even if the library lacks them:** rejected,
  because of the error message at every connection.
- **A global on/off switch for this sync:** replaced by the per-library mode
  (developer, 2026-10-16). It's finer-grained, and it covers future
  write-back too.
- **Also set the Kindle's read state when Calibre's changes:** deferred to
  the Later list (developer, 2026-10-16). It works: a ChangeRequest `update`
  of `readState` to 1 or 3 changes the Library at once. But it's another
  kind of write to the Kindle's catalog. We also haven't checked whether a
  read state set this way gets reported to Amazon, as one set from the
  Library menu is.
- **Percent read through the Content server's HTTP API:** rejected, because
  it's a second connection.
- **A Calibre plugin, for percent read:** deferred to the Later list.

## Consequences
- **To use it**, the user creates the two columns in Calibre (Preferences →
  Add your own columns) and marks the library writable on the Kindle. The
  README documents both steps.
- New config fields per allowed library (`writable`, `read_column`,
  `last_read_column`), and command-line options to set them.
- New state per book in `books.json`: the values last sent by Calibre and
  the values we last reported.
- Changes reach Calibre only when a connection starts, because that's when
  Calibre asks for the book list.
- `p_percentFinished` (and the asset store's `PERCENT_READ`) holds percent
  read, ready for the deferred plugin.
- **Ratings are out of scope** (developer, 2026-10-16). The Goodreads rating
  is asked for on the end-of-book "Before you go" page, which the reference
  device (5.20.1) doesn't show for sideloaded books. That held for a book
  sent with this app and for one sent by Calibre over USB (MTP). The page did
  show for an Amazon book, and for the same sideloaded book on the
  developer's older Kindle (Paperwhite 10th gen, 5.17.1). So on the target
  firmware there's no rating to sync. See
  `research/kindle-library-integration.md`, "Read state probe".

## Implementation notes (2026-10-17)
- **When to reconnect.** The first build dropped the connection as soon as
  `SET_LIBRARY_INFO` showed new columns. Calibre was then waiting for the
  book count, and showed the developer an error dialog ("Device responded
  with incorrect information"). Calibre ignores a failed keep-alive `NOOP`,
  though: it just marks the device disconnected
  (`detect_managed_devices`). So the session now replies to a keep-alive,
  then ends, but only once Calibre has sent nothing but keep-alives for
  10 s. For 5 s after any operation Calibre sends a keep-alive on every
  probe (`SEND_NOOP_EVERY_NTH_PROBE`), and those can fall between two of
  its jobs.
- **Columns removed.** The session that sees it carries on with the columns
  it named. Calibre finds one missing, sends a message to the device (which
  we only log) and skips that sync. The reconnect then names only what
  exists, so sync stops for the missing column, silently. The library stays
  writable, so re-creating the columns resumes it.
- **Calibre skips a book's sync silently on errors.** The GUI wraps
  `synchronize_with_db` in `except Exception: return True`
  (`gui2/device.py`, `set_books_in_library`). In the device test, a book
  with no last-read date was never synced. Calibre writes an empty date as
  the string `"None"` in `user_metadata`, we sent it back as
  `_last_read_date_`, and `parse_date("None")` raised before the read flag
  was looked at. Found with `calibre-debug -g`: the driver logged no sync
  decision at all, and its cached metadata showed the values differed. We
  now send `null` for an empty date.
- **A report counts only once Calibre has taken it.** Because Calibre can
  skip a book without telling us, we no longer treat a Kindle change as
  done once reported. We keep reporting it until Calibre's metadata for the
  book (resent after a sync) shows the value; only then do we go back to
  echoing Calibre's own value (`read_ok`, `last_read_ok` in `books.json`).
  Without this, the test book's change was lost after the skipped sync.
- **Calibre's "bad columns" flag outlives the connection.** Calibre resets
  `have_bad_sync_columns` only when its wireless driver starts, not on each
  connection. Removing a column needs a Calibre restart anyway, which
  resets it.
- **Messages need a reply.** In the device test, deleting the columns made
  Calibre send its "not in calibre's library" message (`DISPLAY_MESSAGE`).
  We logged it without replying, and Calibre's device thread hung waiting,
  so no keep-alives came and the reconnect never happened. The session now
  replies `OK` to every message. That was a bug from Phase 2; read sync is
  just the first thing to trigger one.

