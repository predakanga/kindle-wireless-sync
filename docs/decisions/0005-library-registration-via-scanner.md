# 0005: Register books in the library through the system scanner

- Status: Accepted
- Date: 2026-10-15
- Amended 2026-10-18 (ADR 0016, "KFX books and their keys"): for a book
  whose key only the catalog knows (KFX), the cover can't be written
  before the scanner sees it. It's written once the key is known, and the
  Library shows it the next time it draws the grid.
- Amended 2026-10-18 (see "Deleting through the catalog" at the end):
  deleted books' catalog entries are removed through ChangeRequest, and
  their covers are kept until the entry is gone.

## Context
Requirement 4 says a synced book must appear in the Kindle library right
away. On FW 5.20.1 the library has two stores: `cc.db` and
`ksdk.asset.db`. Triggers and an audit log relay changes from `cc.db` to the
asset DB. `com.lab126.scanner` watches `/mnt/us/documents` and indexes new,
changed and deleted files into both stores within about 2 s. It waits until
the file is closed, and it ignores files whose names it doesn't recognise.
`reScanFile` fails on this firmware, and the ccat upstart job that the
direct-SQL tools restart doesn't exist. Evidence:
`docs/research/kindle-library-integration.md` (probe results).

## Decision
- Rely **only on the scanner** for adding, updating and removing books. Never
  edit `cc.db` or `ksdk.asset.db` directly for the base requirements.
- **New book:** stream to a dot-prefixed temp file in the target directory
  (e.g. `.kws-<n>.part`), then `fsync` and `rename()` it to the final name.
  The scanner never sees a partial book, and an interrupted transfer leaves
  only a temp file, which we clean up on the next start.
- **Replacing an existing book:** the scanner ignores a rename over an
  existing file. So stream to a temp file first, then **copy it over the
  existing file in place** (truncate and write), fsync, and remove the temp
  file. The catalog entry (`p_uuid`) and the reading position are kept. The
  copy is local and fast, which keeps the window for a torn file small.
- **Delete:** remove the book, then its `<name>.sdr/` folder and our cover
  files. The indexer leaves the `.sdr` behind.
- **Cover (required):** the mobi8 extractor only writes a 60×40 "No image
  available" placeholder, so the Library shows a generic tile. Write Calibre's
  thumbnail to `system/thumbnails/thumbnail_<cdeKey>_<cdeType>_portrait.jpg`
  **before** renaming the book into place, so the first time the Library
  draws the book it shows the real cover. Keep a backup in
  `amazon-cover-bug/` and restore missing or placeholder covers on connect,
  as Calibre's USB driver does. `cdeKey`/`cdeType` come from the book's EXTH
  113/501 records, which we parse ourselves.
- Treat `cc.db` as **read-only, lock-tolerant input** for stretch goals: open
  it read-only and retry on `SQLITE_BUSY`.

## Alternatives considered
- `lipc-set-prop com.lab126.scanner reScanFile`: fails on 5.20.1.
- Writing `cc.db` rows ourselves (kindle-series-manager style): fragile. It
  needs the ICU schema hack, relies on a service restart that no longer
  exists, and risks the two stores going out of sync.
- The ChangeRequest API on `127.0.0.1:9101`: not needed for books, since the
  scanner already does this. It's kept as the candidate for collections and
  series (stretch 1).

## Consequences
- Books land in `/mnt/us/documents` or a subfolder of it. The subfolder
  layout is decided later.
- File names need to be stable across updates, so that an update is an
  in-place rewrite and not a delete plus add, which would lose progress.
- Confirmed with real AZW3s: indexed in ~2 s. Without our cover the book
  shows a generic tile. A cover written before the rename shows straight away.
- We need a small MOBI/KF8 EXTH parser (and later a KFX one, if we accept
  KFX).

## Deleting through the catalog (2026-10-18)
Deleting 8 books from Calibre took 40 s or more to clear from the Library,
and looked odd meanwhile (developer, 2026-10-18): the files went at once,
but the scanner removes catalog entries one by one, the first about 17 s
after the delete and then one every 3 to 4 s (research note, "Deleting
through the catalog"). The firmware broke the series up as members
vanished, and the tiles were blank because their covers had already been
deleted with the files.

Now, for a delete from Calibre:
1. Each book's catalog entry (`p_uuid`) is looked up read-only, then the
   file, its `.sdr` folder and any kept cover go at once, and Calibre gets
   its reply.
2. After the whole batch, one ChangeRequest deletes those entries
   (`{"delete": {"uuid": ...}}`, the command already used for series
   containers). The catalog's handler supports deleting a downloaded entry
   by UUID, and its triggers update the asset store. While the scanner holds
   the database ("database is locked", HTTP 503) it's tried again, a second
   apart, up to 10 times. If it still fails, the scanner removes the entries
   in its own time.
3. A book's cover (and its backup) is removed only once its entry has left
   the catalog: straight after a successful ChangeRequest, or when the
   catalog's `updated` event (or a once-a-minute check) shows it gone. Until
   then the tile keeps its cover.
4. The series pass runs straight away, rather than after the 15 s debounce.

Deleting the file first means the scanner can't add the entry back.
Tested on the device with TXT test files (whose keys come from their path,
so nothing else shares them): entries went from both stores within a
fifth of a second, the scanner didn't re-add them, and the Library dropped
the tiles within a couple of seconds. In a batch of 4, one delete waited
about 9 s for the lock; all 4 were gone within 10 s, against about 27 s
for the scanner alone.

The list of covers waiting for their entry to go is kept in
`state/deleted_covers.json`, so a restart in between doesn't leave covers
behind.
