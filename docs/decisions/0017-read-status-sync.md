# 0017: Send read status and last-read date from the Kindle to Calibre

- Status: Proposed
- Date: 2026-10-16

## Context
Phase 5, stretch goal 2: send reading progress and ratings back to Calibre.
The research is in `research/calibre-wireless-protocol.md` ("Stretch 2",
"Where Calibre keeps reading progress", "Can a plugin extend the wireless
driver?"). In summary:

- **Stock Calibre syncs two things both ways:** a read flag and a last-read
  date. The device names two of the user's custom columns in its
  `GET_INITIALIZATION_INFO` reply: `isReadSyncCol` (must be Yes/No) and
  `isReadDateSyncCol` (must be Date). It then reports `_is_read_` and
  `_last_read_date_` in each `GET_BOOK_COUNT` record. Calibre's
  `synchronize_with_db` (`smart_device_app/driver.py`) checks the column
  types once per connection. It compares each device value with the value
  Calibre last sent the device. If they differ, the device's value is
  written into the library. Otherwise Calibre's value stands, and it
  reaches the device in the book's metadata, which the device should then
  show. `_sync_type_: 2` on a record means "this is the first sync since the
  columns were set": the device's value wins if it has one, otherwise
  Calibre's does.
- **Percent read and ratings need a Calibre plugin.** Calibre keeps progress
  in `last_read_positions`, which only its viewer and Content server write.
  A plugin can extend the wireless connection itself, so it would keep a
  single transport. The developer decided (2026-10-16) to **defer the
  plugin** to the Later list. Ratings turned out to be out of scope (see
  Consequences).
- **Calibre has no built-in read or last-read field**, so the user has to
  create the two columns. There's no community convention for their names.
  Every app or plugin found (Calibre Companion, the Kobo plugins) lets the
  user choose. KOReader's Calibre plugin doesn't use these columns at all.
- Calibre sends the library's full column list, with types
  (`fieldMetadata`), in `SET_LIBRARY_INFO` on every connection. We already
  accept that message, and currently ignore it. It arrives *after* the
  handshake in which we name the columns.
- **On the Kindle**, probed on 5.20.1 (`research/kindle-library-integration.md`,
  "Read state probe"):
  - `cc.db` `p_readState` holds the read state: 0 = never set, 1 = marked
    read, 2 = read automatically at the end of the book, 3 = marked unread.
  - `p_lastOpenTime` is set when a book is opened and when it's closed, and
    is unset before the first open. `p_lastAccess` is also set when the file
    is indexed, so it isn't a reading signal.
  - A ChangeRequest `update` with `readState` 1 or 3 sets the state. The
    asset store follows through its triggers, and the Library tile changes
    at once.

## Decision
**Fixed column names for now.** We use **`#kindle_read`** (Yes/No) and
**`#kindle_last_read`** (Date), and document them in the README. The
`kindle_` prefix keeps them apart from a user's own `#read`, which may have
another type or be owned by another tool. Picking columns from a list in our
app comes with the UI redesign (Later list, "Review the UI"). The
`fieldMetadata` list makes that possible without typing.

**Each allowed library is read-only or read-write.** The mode is stored on
the library's allowlist entry in `config.json` (ADR 0003), as
`"access": "read-only"` or `"read-write"`.
- A **read-only** library only sends to the Kindle. We never name sync
  columns for it, so nothing we report changes that library.
- A **read-write** library may also receive changes from the Kindle: read
  status and last-read date now, and whatever a later Calibre plugin adds.
- **The default is read-only.** Writing into a user's Calibre library should
  be a deliberate choice, made per library: a user may allow a household
  library to send books without letting this Kindle change it. Existing
  entries without the field are read-only.
- **Setting it:** for now, only with the command line: `--read-write` on
  the `allow` command, plus a way to change it for an already allowed
  library (developer, 2026-10-16). A toggle on the library's entry in the
  app comes with the UI redesign (Later list).

**Opt in by creating the columns.** This applies only to read-write
libraries. Calibre reports every bad sync column to the device as a
message, on every connection. So we name a column in the handshake only
once we've seen it, with the right type, in that library's
`fieldMetadata`. We record which columns each library has, keyed by the
library's UUID, and update the record on every connection.
- The connection where the columns first appear syncs nothing. The next one
  syncs. To avoid waiting, the daemon reconnects straight away when it sees a
  sync column appear (a single reconnect, with backoff as usual).
- The first sync after a column appears sends `_sync_type_: 2` for each
  book, so neither side's existing values are wiped.
- If a column disappears, we stop naming it from the next connection.

**What we report per book:**
- `_is_read_`: true for `p_readState` 1 or 2, false for 3. A 0 (never set)
  gives the Kindle no opinion, so we report Calibre's last value.
- `_last_read_date_`: `p_lastOpenTime` as an ISO date, or Calibre's last
  value if the book has never been opened on the Kindle.

Reads are read-only `sqlite3` queries on `cc.db` (ADR 0005), at the start of
each session, when we build the `GET_BOOK_COUNT` reply.

**One direction: Kindle → Calibre** (developer, 2026-10-16). Changes made
on the Kindle reach Calibre. Changes made in Calibre stay in Calibre: we
don't write the Kindle's read state. Setting it is possible (see Context),
and is on the Later list.

**Don't undo Calibre's edits.** Calibre's rule assumes the device shows the
value Calibre sent, unless the user changed it on the device. A device
that just reported its own unchanged value would overwrite every edit made
in Calibre. So we store, per book, the read flag and date Calibre last sent
us (in `books.json`, from the book's `user_metadata`), and the Kindle's
values when we last reported them.
- If the Kindle's value has changed since our last report, we report the
  Kindle's value, and Calibre takes it.
- Otherwise we report Calibre's last value, so a change made in Calibre isn't
  undone by the Kindle's older value.
- So the two sides can differ: a book marked read in Calibre stays unread
  on the Kindle until it's marked there. That's accepted for now.

**No separate off switch.** A read-only library never syncs. That also
covers a user whose library has columns with these names that mean
something else.

## Alternatives considered
- **Pick the columns from a list in our app:** preferred long-term, but
  deferred to the UI redesign. Fixed names need no UI.
- **Type the column names into our app:** typing on the e-ink keyboard, and
  easy to get wrong.
- **Guess columns from their names** (a Yes/No column called "read"):
  rejected. A wrong guess writes into a column that means something else.
- **Also setting the read flag on the Kindle when Calibre's changes**
  (ChangeRequest `update` of `readState` to 1 or 3): deferred to the Later
  list (developer, 2026-10-16). It works on the device, but it's another
  kind of write to the Kindle's catalog. We also haven't checked whether a
  read state set this way is reported to Amazon, as a menu "Mark as read"
  is in the telemetry buffer.
- **A global `"read_sync": false` switch:** replaced by the per-library
  mode (developer, 2026-10-16). It's finer-grained, and it also covers
  future write-back, such as a plugin.
- **Always name the columns, even when the library lacks them:** rejected.
  Calibre shows an error message on the Kindle on every connection for users
  who haven't created them.
- **Percent read through the Content server** (`POST
  /book-set-last-read-position/...`): rejected. It's a second transport.
- **A Calibre plugin, for percent read:** deferred to the Later list.

## Consequences
- To sync, the user creates two columns in Calibre (Preferences → Add your
  own columns) with exact lookup names, and marks the library read-write on
  the Kindle. The README documents both steps. The column headings can be
  anything.
- New config field per allowed library (`access`), and command-line options
  to set it. The app toggle, and the API call it needs (ADR 0011), wait for
  the UI redesign.
- New state in `books.json` per book: the last values from each side.
- Sync happens only while a session is open, at its start. That's when
  Calibre asks for the book list. A change on the Kindle reaches Calibre on
  the next connection.
- The probes this decision needed were run on 2026-10-16 (see Context).
- `p_percentFinished` (with the asset store's `PERCENT_READ`) holds percent
  read, ready for the deferred plugin.
- **Ratings are out of scope** (developer, 2026-10-16). The Goodreads
  rating is asked for on the end-of-book "Before you go" page, which the
  reference device (5.20.1) doesn't show for sideloaded books. That held
  for a book sent with this app, and for one sent by Calibre over USB
  (MTP). It did show for an Amazon book, and for the same sideloaded book
  on the developer's older Kindle. So on the target firmware there's no
  rating to sync. See `research/kindle-library-integration.md`, "Read state
  probe".
