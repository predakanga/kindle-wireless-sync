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
  suggests a list. ("Paired" was the other candidate; see Open questions.)

**Config.** `config.json` holds one optional `library` object instead of the
`allowed` list: UUID, name, last host and port, broadcast hostname,
password, and the read-sync settings from ADR 0017 (`writable`,
`read_column`, `last_read_column`). The `seen` list stays: Find libraries
still lists what it found, for the user to link one.
- **Migration** (on load, written back at the next save): an old `allowed`
  list becomes `library`. If it has several entries, keep the one whose UUID
  matches `driveinfo.json`'s `last_library_uuid` (the library Calibre last
  synced with), or else the first, and log the ones dropped.

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
libraries inside Calibre) is refused, as now, and recorded as seen. The app
can offer to link it instead, with the same confirmation.

### What happens to `books.json`
Two ways to forget the old library's metadata:
1. **Delete `books.json`** and let adoption (ADR 0013) rebuild it at the
   next session. That's simple, but adoption only recovers books whose file
   carries a Calibre UUID: AZW3 and MOBI (EXTH 112/113), or anything listed
   in `metadata.calibre` from a USB transfer. **PDF, TXT and KFX books sent
   wirelessly have neither, so Calibre would never see them again.** They'd
   stay on the Kindle, readable, but invisible to Calibre, which couldn't
   update or delete them.
2. **Keep each record's identity (lpath, size, `_kws`), and drop the
   Calibre-library parts** (title stays, but series, `user_metadata` and
   read-sync state go). The new library's Calibre then pairs each book by
   UUID or title and author, and resends its own metadata for matches.
   Books it can't match show as on the device but not in the library, as
   they would after a USB transfer.

**Proposed: option 2**, because option 1 silently loses books in three of
the six formats we accept.

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
  - `config` holds `Option<LinkedLibrary>` and does the migration.
  - The daemon's candidate list, password-failure set and status text
    become single-library.
  - `Library` gets a reset for re-linking (driveinfo, columns, per-book
    metadata).
  - The session's admission is unchanged in shape.
  - It's a net reduction in code.
- **Command line:** `allow` becomes `link` (with `--writable`/`--read-only`,
  and a confirmation flag such as `--replace` when another library is
  linked), and the app's Forget becomes `unlink`. The API endpoints
  (`/allow`, `/forget`) are renamed to match (ADR 0011). There's no
  compatibility shim, as the only users are the developer's Kindle and our
  own app, upgraded together.
- **App:** a single "Calibre library" section: the linked library, or Find
  libraries and Add server if there's none. A found library can be linked,
  with a confirmation if it replaces one.
- **README and CHANGELOG** say "link" throughout; the change in behaviour
  (one library) gets a changelog entry.
- **Tests:** config migration (none, one, several), re-linking the same and
  a different library, and the daemon only trying the linked library.

## Open questions
- "Link" or "pair"? "Pair" is familiar from Bluetooth, which suits a
  one-to-one relationship, but suggests a handshake on both sides, and
  Calibre has nothing to approve.
- Should re-linking a different library clear covers we backed up? They're
  keyed by the Kindle's content key, not by library, so probably not.
- Does Calibre's wireless driver care that a device it knew under one
  library now shows up with another library's books? Clearing
  `driveinfo.json` should make it a new device to Calibre; to verify.
