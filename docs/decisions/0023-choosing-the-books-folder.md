# 0023: Choosing the folder received books go into

- Status: Accepted (developer, 2026-10-19)
- Date: 2026-10-18
- Amends: [0005](0005-library-registration-via-scanner.md)
  ("the subfolder layout is decided later") and
  [0020](0020-one-screen-pages-and-checking-a-library.md) (a new page).

## Context
Every received book goes straight into `/mnt/us/documents`, under the name
Calibre's template gives it (`{title} - {authors} ({id})` by default), so
they all sit in one flat folder (`PLAN.md`, Later item 12). The developer
wants a setting for the folder, chosen with a folder picker rather than by
typing a path, and books already received should stay where they are
(2026-10-18).

What applies:
- **The Kindle indexes subfolders of `documents`.** A TXT in a subfolder
  was catalogued with the same flags as one in `documents` itself, on
  5.20.1 (`research/kindle-library-integration.md`, "Books in subfolders
  are indexed"). Nothing outside `documents` is indexed for the Library.
- **The indexer makes a `.sdr` folder** next to each book, in subfolders
  too.
- **Calibre names a book by its lpath**, relative to the device's books
  folder, and works it out from its template on every send, updates
  included (`upload_books`, `_create_upload_path` in
  `smart_device_app/driver.py`). It doesn't reuse the lpath the device
  reported.
- **The device may rename a book as Calibre sends it.** With
  `canSupportLpathChanges`, Calibre takes the `lpath` in our `OK` reply to
  `SEND_BOOK` and records that one (`_put_file`). Later requests, such as
  deleting the book or its metadata cache entry, use it. This needs
  `wantsSendOkToSendbook`, which Calibre sets because we declare
  `canSendOkToSendbook`.
- **An update today is matched by lpath:** the same lpath means an in-place
  rewrite (ADR 0005), which keeps reading progress.

## Decision
**The books root stays `/mnt/us/documents`, and the chosen folder becomes a
prefix on new books' lpaths.** `config.json` gets `books_folder`, a path
relative to `documents` (empty for `documents` itself). It's one setting
for the device, like series, not per library. lpaths stay relative to
`documents`, so `books.json` and `driveinfo.json` don't change shape, and
books already received keep their lpaths, wherever they are.

**Sending a book (`SEND_BOOK`):**
1. If we hold a book with the same Calibre `uuid` and the same format
   (extension), this is an update. Write it in place under *that* book's
   lpath, and reply with it.
2. Otherwise it's a new book: its lpath is `books_folder/` plus Calibre's
   lpath, created as needed. Reply with that.
3. If Calibre doesn't want an `OK` (it always does in 9.16), we can't rename
   the book, so it keeps Calibre's lpath, as now.
The result goes through the same validation as any lpath (ADR 0003).

Matching by uuid also changes renames: a book renamed in Calibre is now
updated in place under its old file name instead of arriving under a new
one. The Kindle shows the title from the book's own metadata, not the file
name, and reading progress is kept.

**Changing the folder moves nothing.** Only books received afterwards go
into the new folder. Folders emptied by deletions are left alone; they may
be the user's own.

**The picker** (a page in the app, from Settings: "Books folder", showing
the current one, with Change):
- It starts at `documents`, the top: nothing outside it can be chosen.
- It lists the subfolders of the folder shown, a page at a time
  (one screen per page, ADR 0020), leaving out hidden folders (starting
  with `.`) and `.sdr` folders. Tapping one opens it. Below `documents`,
  the list starts with a row "Up to *parent*", in italics, which opens the
  folder above (the developer's suggestion, 2026-10-19). The header's Back
  leaves the picker without saving.
- **Use this folder** saves it. **New folder** asks for a name (the
  on-screen keyboard), creates it inside the folder shown, and opens it.
- A folder whose name wouldn't pass lpath validation (ADR 0003) can't be
  chosen; the picker says why.

**API** (ADR 0011), with paths in JSON bodies rather than URLs, since
folder names can need encoding: `POST /folders` with `path` lists its
folders (each with a `problem` if it can't be used), `POST /folders/new`
with `path` and `name` creates one, and `POST /books-folder` with `path`
saves the setting. `/status` reports `books_folder`. Every path is checked
to stay inside `documents` after resolving symlinks (`folders.rs`).

## Alternatives considered
- **Make the chosen folder the books root,** so lpaths are relative to it.
  Books already received would then need absolute paths, or lpaths
  relative to `/mnt/us`, in our state, and Calibre's lpaths for them would
  change meaning. The prefix keeps everything relative to one root.
- **Move existing books into the new folder.** Their lpaths would change,
  so Calibre's cache and the Kindle's catalog entries would have to follow,
  and reading progress depends on the file staying put. The developer
  chose to leave them.
- **Type the folder's path.** Error-prone on the on-screen keyboard; the
  developer chose a picker. Typing remains only for a new folder's name.
- **Folders outside `documents`.** The Library doesn't index them.

## Consequences
- `receive_book` looks up the incoming uuid and format before preparing the
  file, and replies with the lpath it chose.
- New API calls, a picker page and a Settings row; the picker needs paging
  for a documents folder with many subfolders.
- To check on the device: a book received into a subfolder appears in the
  Library and gets its cover; updating it rewrites it in place; deleting it
  from Calibre removes it and its `.sdr`; changing the folder leaves older
  books where they are and updates still find them.
- The developer confirmed that nothing outside `documents` can be chosen
  (2026-10-19).

## Implementation notes (2026-10-19)
- **Checked on the reference Kindle** (5.20.1) with the developer's
  Calibre. With the folder set to `kws-folder-test` (made with the
  picker's New folder): a new book (*The Rithmatist*) went into it, was
  catalogued and got its cover; a book already in `documents` (*The Alloy
  of Law*) was rewritten in place there, keeping its `.sdr`, with no second
  copy; deleting the new book from Calibre removed it, its `.sdr` and its
  catalog entry.
- **Deleting the folder's only book first removed the folder too.**
  `delete_book` tidies emptied parent folders up to `documents`, for the
  per-author folders Calibre's templates can make. A book now records the
  books folder it was placed in (`_kws.folder` in `books.json`, kept
  across updates and key lookups), and the tidying stops there. Retested:
  the folder stayed. Books received before this have no record of it and
  tidy up to `documents`, as before.
- **The picker pages ten folders at a time.** The real app was checked end
  to end in headless Chrome against the service on the Mac, driven over
  the DevTools protocol (hidden and `.sdr` folders left out, a link out of
  `documents` listed as unusable, paging, Up, New folder, Use this folder).
