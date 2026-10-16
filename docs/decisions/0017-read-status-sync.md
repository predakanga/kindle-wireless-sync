# 0017: Sync read status and last-read date with Calibre

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
  plugin** to the Later list. Ratings are also unprobed on the Kindle (see
  Consequences).
- **Calibre has no built-in read or last-read field**, so the user has to
  create the two columns. There's no community convention for their names.
  Every app or plugin found (Calibre Companion, the Kobo plugins) lets the
  user choose. KOReader's Calibre plugin doesn't use these columns at all.
- Calibre sends the library's full column list, with types
  (`fieldMetadata`), in `SET_LIBRARY_INFO` on every connection. We already
  accept that message, and currently ignore it. It arrives *after* the
  handshake in which we name the columns.
- **On the Kindle** (`research/kindle-library-integration.md`, "Progress,
  read state and rating"): `cc.db` has `p_lastAccess` and `p_lastOpenTime`
  (epoch seconds), and a legacy `p_readState` (0/1/2). The asset store has
  `READ_STATE`. Modern "Mark as read" was seen in `fmcache.db`, a rolling
  upload buffer, which is unreliable on an offline device. **We haven't yet
  checked which field "Mark as read" and finishing a book change on 5.20.1.**

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
- `_is_read_`: the Kindle's read flag, from the field that "Mark as read"
  and finishing a book set. That's to be confirmed by a probe before we
  build.
- `_last_read_date_`: when the book was last opened on the Kindle
  (`p_lastOpenTime`, falling back to `p_lastAccess`), as an ISO date, or
  null if the book was never opened.

Reads are read-only `sqlite3` queries on `cc.db` (ADR 0005), at the start of
each session, when we build the `GET_BOOK_COUNT` reply.

**Calibre → Kindle.** Calibre's rule assumes the device shows the value
Calibre sent unless the user changed it on the device. So we store, per
book, the read flag and date Calibre last sent us (in `books.json`, from
the book's `user_metadata`), and the Kindle's values when we last reported
them.
- If the Kindle's value has changed since our last report, we report the
  Kindle's value, and Calibre takes it.
- Otherwise we report Calibre's last value, so a change made in Calibre isn't
  undone by the Kindle's older value.
- Whether we also **set** the read flag on the Kindle when Calibre changes
  it depends on the probe: is there a ChangeRequest `update` the firmware
  honours? If there is, we use it. If not, Calibre's value stays in Calibre
  only, and the Kindle's flag changes only on the Kindle.

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
- **A global `"read_sync": false` switch:** replaced by the per-library
  mode (developer, 2026-10-16). It's finer-grained, and it also covers
  future write-back, such as a plugin.
- **Always name the columns, even when the library lacks them:** rejected.
  Calibre shows an error message on the Kindle on every connection for users
  who haven't created them.
- **Percent read through the Content server** (`POST
  /book-set-last-read-position/...`): rejected. It's a second transport.
- **A Calibre plugin, for percent read and ratings:** deferred to the Later
  list.

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
- **Probes before building**, on the reference device:
  1. "Mark as read", "Mark as unread" and finishing a book: which `cc.db` or
     asset-store fields change, for a sideloaded book.
  2. Whether a ChangeRequest `update` can set the read flag, and the Library
     shows it.
  3. Which of `p_lastOpenTime` and `p_lastAccess` tracks opening a book,
     rather than other access (such as indexing or a cover refresh).
- **Ratings stay open.** The developer points out that the Kindle prompts
  for a Goodreads rating after a book is finished, so a rating may be stored
  somewhere. That needs a probe with a rated book (sideloaded and, for
  comparison, an Amazon one). It only matters once the plugin is picked up.
