# 0018: Link the Kindle to one Calibre library

- Status: Accepted
- Date: 2026-10-17
- Amended by [0020](0020-one-screen-pages-and-checking-a-library.md)
  (2026-10-18): the app checks a library (password, columns) before linking
  it, and links it with the settings chosen.

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
- deletes `driveinfo.json`, so the new library's Calibre sees a new device
  and starts its metadata cache afresh (checked; see "Clearing
  `driveinfo.json` makes a new device"). The whole file goes, not just the
  device UUID. The only other field that survives a connection is Calibre's
  `device_name`, which the next session sets again to the Kindle's current
  name (developer, 2026-10-17);
- clears `columns.json` and every book's read-sync state, and resets
  `writable` to `false` (a fresh link starts read-only, as ADR 0017
  requires);
- **forgets the old library's metadata for each book, but keeps the book
  records** (see "What happens to `books.json`" below).

Re-linking the same library (same UUID), for example after its address or
password changed, resets nothing.

**The daemon runs the reset, before its next session** (developer,
2026-10-17). It holds `books.json` in memory, and `link` can run as a
separate process, so linking only changes `config.json`. A new link starts
with `writable` off, because it's a new `library` object. The library the
Kindle's state belongs to is the `last_library_uuid` in `driveinfo.json`.
Calibre writes it on every connection (`_update_driveinfo_record`). Before
each session, the daemon compares it with the linked library:
- **No `driveinfo.json`:** nothing to reset. This is a fresh install, or a
  Kindle just after a reset.
- **It matches the linked library:** no reset.
- **It names a different library:** reset. `driveinfo.json` is deleted
  last, so an interrupted reset runs again.
- **It's empty:** no reset, and a warning in the log. Resetting here could
  repeat before every session if a Calibre version always left it empty.
  Each repeat would make a new device and rebuild every series. Calibre
  9.16.0 fills it from the library its GUI loaded
  (`set_current_library_information` in `gui2/device.py`), before any
  device connects.

The alternative was a marker file of our own. It would avoid depending on
Calibre's field, but it's one more state file, and existing installs would
still need the `driveinfo.json` check the first time.

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
   Calibre-library parts** (title and series stay, but `user_metadata` and
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
  - **`series` and `series_index`** (amended 2026-10-18, developer): they
    group the Kindle's library, which is useful whatever library is
    linked. The first version dropped them, so replacing a library broke
    up every series until the new library resent its metadata, and kept
    them broken up for books it doesn't have.
- **Dropped:** everything else Calibre sent, which belongs to the old
  library: tags, `user_metadata` (custom columns, including the read-sync
  columns), identifiers, comments and the rest, and `_kws`'s read-sync
  state.

Series follow automatically. The series pass (ADR 0016) leaves them as they
are, and changes a book's series once the new library resends its metadata:
to that library's series, or none. Books the new library doesn't have keep
their series.

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

## Clearing `driveinfo.json` makes a new device (checked 2026-10-17)
This was an open question. Calibre's source (9.16.0) and a test with the
emulated Kindle and the Calibre harness both answer it: yes.
- **Source.** `smart_device_app/driver.py`, `get_device_information`: when
  the `device_info` we send has no `device_store_uuid`,
  `_update_driveinfo_record` creates a new `uuid4`. Calibre returns it in
  `SET_CALIBRE_DEVICE_INFO`, and we store it. The metadata cache file is
  named after that UUID (`wireless_device_<uuid>_metadata_cache.json`,
  `_read_metadata_cache`), so a new UUID starts with an empty cache.
  Outside the driver, only the `connected_device_uuid()` template function
  reads the UUID. Nothing in Calibre reads `last_library_uuid`.
- **Test.** The emulator (`pw6-5201-jb`, run with `-snapshot`) linked to the
  harness library, holding one book:
  - A normal session used device UUID `8868ac0e-…`, and Calibre got every
    book from its cache ("Need count from device 0").
  - With `driveinfo.json` removed, the next session got a new UUID,
    `b8a0e635-…`. Calibre loaded 0 cache items, asked the device for full
    metadata for its 1 book, and wrote a new cache file. The book still
    matched its library record by UUID.
- **Why it matters.** Calibre's cache is per device, not per library, and
  is keyed on book UUID and `lpath`, and checked against `last_modified`.
  The records we keep keep all three. If the device UUID stayed the same,
  the next library opened in the same Calibre would get cache hits for
  those books. Calibre would then show the old library's series, tags and
  columns without asking the device. A new device UUID avoids that.
- **Side effects.** The old cache file stays in Calibre's cache folder, and
  nothing removes it. It's small, and it's Calibre's to manage.
- **Only two fields survive a connection.** `_update_driveinfo_record`
  rewrites `last_library_uuid`, `calibre_version`, `date_last_connected`,
  `location_code` and `prefix` on every connection. It keeps only
  `device_store_uuid` and `device_name` if they are present. Clearing the
  file therefore differs from removing just the UUID only in
  `device_name`. That field is Calibre's (ADR 0015). Calibre fills it on
  first contact, and we supply the Kindle's name then. Only
  `SET_CALIBRE_DEVICE_NAME` (`set_driveinfo_name`) changes it after that,
  and in 9.16.0 nothing in Calibre's GUI calls it, so only a plugin could.
  The `connected_device_name()` template function is the only place
  Calibre shows it.

## Implementation notes (2026-10-17)
- **`link` without HOST** changes the linked library's settings
  (`--password`, `--writable`/`--read-only`) without contacting Calibre.
  ADR 0017's `allow UUID --writable` worked without Calibre running, and
  this keeps that. Linking by HOST needs Calibre to answer.
- **`link`'s port is optional.** Without one, it uses Calibre's default
  wireless port, 9090.
- **`/link`** takes `{"uuid"}` for a found library, linked at the address
  where it was found, without contacting it again. It takes `{"host",
  "port", "password"?, "uuid"?}` to link by address. Either form can add
  `"replace": true`. Without it, linking a different library replies
  `{"ok": false, "needs_replace": true, "linked", "library"}`. After the
  user confirms, the app sends the same request again with `"replace":
  true` and the answering library's `uuid`. A by-address link is probed
  again, so a different library answering by then is refused, not linked.
- **Replacing or unlinking a library cuts the open session**, so the old
  library gets no more syncs. `/link` and `/unlink` do it directly, as
  `/password` does. `link --replace` and `unlink` on the command line run
  as a separate process, so they ask the daemon through the API's
  `/reconnect` (developer, 2026-10-17). If no daemon is listening, there's
  no session to cut.
- **`find` on the command line** prints a ready-made `link` command for
  each library it found, with `--uuid`.
- **Found libraries are listed without the linked one**, and a library
  leaves the list once it's linked.
- **A reset record's `last_modified` is its file's modification time**, as
  adoption does (ADR 0013), not the old library's date. Calibre resends a
  matched book's metadata only when the library's `last_modified` differs
  from the device's (`gui2/device.py`, `set_books_in_library`). With the old
  date kept, re-linking the old library would match those books by UUID
  but never fill their records in again. Tested on the emulator with the
  harness, under Calibre's default "Manual management": with the old date,
  Calibre sent metadata for 0 books, and *Update cached metadata on
  device* didn't change that. With the file's date, it sent metadata for
  the book as soon as it connected.
- **Tested on the emulator** (`pw6-5201-jb`, 2026-10-17):
  - An upgrade from the allowlist starts unlinked.
  - Linking by address connects. It doesn't reset when `driveinfo.json`
    already names that library.
  - Linking without `replace` changes nothing and returns `needs_replace`.
  - Confirming resets before the session: new device UUID, new Calibre
    cache, and the book still matched by UUID, with no error dialogs.
  - The app first didn't open on the emulator: the install had left its
    files readable only by root (ADR 0012, "Hooks inherit the caller's
    umask"). With that fixed, the app's replace flow worked end to end. A
    library answering at the linked address was listed as found. Link
    asked first, and confirming reset the state and connected without
    restarting the service. Calibre then resent the book's metadata.
  - That flow found a bug, now fixed. The address was still on the
    daemon's skip list for libraries that aren't linked, so the newly
    linked library wasn't tried until Wi-Fi cycled. Linking another
    library now clears the list.
  - `unlink` on the command line, during an idle session, cut the session
    through `/reconnect`. Calibre showed no error dialog.

## Calibre switching libraries (2026-10-18)
Calibre keeps a wireless device connected when the user switches to another
library. It only sends the device the new library's `SET_LIBRARY_INFO`
(`set_current_library_information` in `gui2/device.py`). Until then the
session checked the library only in the handshake, so a Kindle stayed
connected to whichever library Calibre switched to: books sent from it
arrived, and read sync could write to its columns. That broke this
decision's promise, and the allowlist (ADR 0003) had the same gap.

Now a `SET_LIBRARY_INFO` naming another library ends the session at the
next quiet keep-alive (`SessionEnd::LibraryChanged`), the way new sync
columns do (ADR 0017), and that library's columns aren't recorded as the
linked one's. The daemon treats it like another library answering at that
address: it goes into the found list and the address is skipped, and the
status says which library Calibre serves and to switch back, then Connect.
Connect now (and so the app's **Connect**) also retries skipped addresses,
since Calibre may have switched back.

Tested with the harness Calibre and the service on the Mac: switching to a
second library sent its library info at once; the session left about 16 s
later with no dialog; the status named the other library; switching back
and Connect reconnected. Until the session ends, Calibre can still send
books from the other library. Ending sooner is on the Later list
(`PLAN.md`, item 10).
