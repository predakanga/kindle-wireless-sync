# 0018: Link the Kindle to one Calibre library

- Status: Proposed
- Date: 2026-10-17

## Context
ADR 0003 made the Kindle connect only to Calibre libraries the user has
allowed, and ADR 0008 described how they get allowed (Find libraries, Add
server). Both called it an **allowlist**, and the config stores a list
(`allowed`). Nothing ever decided that more than one library should be
supported. A list followed from the word "allowlist" (developer,
2026-10-17).

With two allowed libraries, A and B, the Kindle can be connected to A in one
session and B in the next. But everything we keep is per Kindle, not per
library: `books.json`, `driveinfo.json` (the device identity Calibre caches
metadata under), our series, and read-sync state. That causes problems:
- **Read sync can write into the wrong library** (ADR 0017). Calibre pairs
  a device book with a library book by UUID, then by title and author
  (`gui2/device.py`, `set_books_in_library`). When B is connected, a book
  sent from A that has a same-titled book in B gets paired with it, and
  if B is writable, its read status lands in B's column.
- B's Calibre sees A's books as "on the device, not in the library", and
  can delete them.
- Series are built from every book on the Kindle, whichever library sent
  it.

Supporting several libraries properly would mean recording which library
sent each book and scoping series and read sync to it. That's more code in
the hardest-to-test parts, for a case the developer doesn't expect anyone to
want, including themselves. Multiple libraries also complicate the UI (a
list of libraries, each with its own password and settings).

## Decision
**The Kindle is linked to at most one Calibre library.** There is no
allowlist any more: the user **links** the Kindle to a library, and the
daemon only ever connects to that library. The security property of ADR
0003 is unchanged: no other library can drive a session.

**Terminology.** "Link" in the app, the command line, the API and the docs:
- *Link* a library (from Find libraries or Add server), *Unlink* it.
- "Linked library" in status text and settings.
- Not "connected", which already means an open session. Not "allowed", which
  suggests a list. Not "paired", which suggests both sides approve it, and
  Calibre has nothing to approve (developer, 2026-10-17).

**Config.** `config.json` holds one optional `library` object instead of the
`allowed` list: UUID, name, last host and port, broadcast hostname,
password, and the read-sync settings from ADR 0017 (`writable`,
`read_column`, `last_read_column`), plus the `series` switch. That's all:
the config holds the user's settings, and the linked library's last
address, which reconnecting needs.

**Found libraries are kept in memory, not in the config** (developer,
2026-10-17). The `seen` list only drove the UI and the `find` → `allow`
command pair. The daemon now keeps what Find libraries found, and any other
library that answered at the linked address, in memory. The app's Find and
Link both go through the daemon (ADR 0011), so they share it. A restart
forgets it; the user runs Find libraries again. Find no longer rewrites
`config.json` on every run.

**Linking from the command line goes by address**, so it needs no saved
list: `link HOST [PORT] [--uuid UUID]` probes that address and links the
library that answers. With `--uuid`, it refuses if a different library
answers, which closes the race between `find` and `link`, for example when
the user switches libraries in Calibre in between (developer, 2026-10-17).
`find` only prints what it found, with each library's address and UUID, so
its output can be copied into `link`. `link` replaces both `allow` and
`add-server`, with `--password`, `--writable`/`--read-only`, and
`--replace` to confirm replacing a different linked library.
- **No migration** from the `allowed` list: the app hasn't been shared
  with anyone yet (developer, 2026-10-17). The developer's Kindle is
  re-linked by hand after upgrading.

**Linking a different library resets what belonged to the old one.** It's a
deliberate act, so the app asks first, and says that books stay on the
Kindle. The daemon then:
- clears `driveinfo.json`, so the new library's Calibre sees a new device
  and starts its metadata cache afresh;
- clears `columns.json` and every book's read-sync state, and resets
  `writable` to `false` (a fresh link starts read-only, as ADR 0017
  requires);
- **forgets the old library's metadata for each book, but keeps the book
  records** (see "What happens to `books.json`" below).

Re-linking the same library (same UUID), for example after its address or
password changed, resets nothing.

**Unlinking** stops the daemon connecting anywhere. Books, records and
series stay as they are, so linking the same library again picks up where it
left off.

**A different library answering at the linked address** (the user switched
libraries inside Calibre) is refused, as now, and added to the daemon's
found libraries. The app can offer to link it instead, with the same
confirmation.

### What happens to `books.json`
Two ways to forget the old library's metadata:
1. **Delete `books.json`** and let adoption (ADR 0013) rebuild it at the
   next session. That's simple, but adoption only recovers books whose file
   carries a **Calibre book UUID**: AZW3 and MOBI (EXTH 112/113), or
   anything listed in `metadata.calibre` from a USB transfer. (This is not
   the Kindle's content key, which for PDF and TXT is `"*" +
   sha1(path)`. That key ties a file to its Library entry, not to a Calibre
   book.) **TXT files can't carry a Calibre UUID, so a TXT sent wirelessly
   would never be seen by Calibre again.** It would stay on the Kindle,
   readable, but Calibre couldn't update or delete it. PDF and KFX are
   unknown: Calibre may write its UUID into a PDF's metadata, and KFX was
   never explored, but adoption reads neither today.
2. **Keep each record's identity (lpath, size, `_kws`), and drop the
   Calibre-library parts** (title stays, but series, `user_metadata` and
   read-sync state go). The new library's Calibre then pairs each book by
   UUID or title and author, and resends its own metadata for matches.
   Books it can't match show as on the device but not in the library, as
   they would after a USB transfer.

**Proposed: option 2**, because option 1 silently loses TXT books, and
possibly PDF and KFX ones, unless adoption learns to read those formats.
Agreed by the developer (2026-10-17). Exactly what each record keeps:
- **Kept:** `lpath`, `size`, `last_modified`, the Calibre book `uuid`,
  `title` and `authors`, and our private `_kws` except its read-sync state
  (so the content key, which covers and series use).
  - **The UUID matters.** Calibre's book list carries one per book, and
    Calibre keys its device metadata cache on UUID and `lpath`. The new
    library won't match it, since book UUIDs are unique per library, so it
    falls back to title and authors. But if the user links the old library
    again, it matches every book exactly again.
  - **Title and authors** are what the new library matches on.
- **Dropped:** everything else Calibre sent, which belongs to the old
  library: series, tags, `user_metadata` (custom columns, including the
  read-sync columns), identifiers, comments and the rest, and `_kws`'s
  read-sync state.

Series follow automatically. The next series pass (ADR 0016) sees no series
in the records, dissolves ours, and rebuilds them from the new library's
metadata once Calibre resends it.

## Alternatives considered
- **Keep several allowed libraries, and scope state per library:** record
  the sending library per book, and only sync and group its own books.
  Rejected as more complexity for a case nobody is expected to want.
- **Keep the list, and warn against read sync with more than one library:**
  cheapest, but it leaves the trap in place.
- **One library, but keep the word "allowlist":** "allowed" still suggests
  a list, and invites the same mistake again.

## Consequences
- **Supersedes** the "allowlist" parts of ADR 0003 (the list of allowed
  UUIDs) and ADR 0008 (connecting to any allowed library, per-library
  password failures). Their trust rules stay: unknown libraries are never
  connected to in the background, and every connection checks the library
  UUID. On acceptance, those ADRs get "Amended by 0018" notes, and ADR 0017's
  "each allowed library" wording becomes "the linked library".
- **Code:**
  - `config` holds `Option<LinkedLibrary>`.
  - The daemon's candidate list, password-failure set and status text
    become single-library.
  - `Library` gets a reset for re-linking (driveinfo, columns, per-book
    metadata).
  - The session's admission is unchanged in shape.
  - It's a net reduction in code.
- **Command line:** `link` (see Decision) replaces `allow` and `add-server`,
  and `unlink` replaces the app's Forget; `find` only prints. **API** (ADR
  0011): `/link` (by UUID, from the daemon's found libraries, or by
  address, as Add server does now) and `/unlink` replace `/allow`,
  `/add-server` and `/forget`; `/status` reports the found libraries from
  memory. There's no compatibility shim, as the only users are the
  developer's Kindle and our own app, upgraded together.
- **App:** a single "Calibre library" section: the linked library, or Find
  libraries and Add server if there's none. A found library can be linked,
  with a confirmation if it replaces one.
- **README and CHANGELOG** say "link" throughout; the change in behaviour
  (one library) gets a changelog entry.
- **Tests:** re-linking the same and a different library, `link --uuid`
  refusing a different library, the daemon only trying the linked library,
  and found libraries surviving between API calls but not restarts.
- **Covers we backed up are kept** when linking a different library
  (developer, 2026-10-17). They're keyed by the Kindle's content key, not
  by library.

## Open questions
- Does clearing `driveinfo.json` really make the Kindle a new device to
  Calibre, with a fresh metadata cache? Explore on the device before
  building the reset (developer, 2026-10-17: unknown).
