# 0005: Register books in the library through the system scanner

- Status: Proposed
- Date: 2026-10-15

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
- **Cover:** write Calibre's thumbnail to
  `system/thumbnails/thumbnail_<cdeKey>_<cdeType>_portrait.jpg` and a backup
  in `amazon-cover-bug/`, and restore missing ones on connect, as Calibre's
  USB driver does. Whether to skip this when the extractor produces its own
  thumbnail depends on the pending AZW3 probe.
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
- Still to confirm with a real AZW3: the mobi8 extractor's timing and its
  thumbnail behaviour.
