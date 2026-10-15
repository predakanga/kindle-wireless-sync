# 0013: Adopt books that Calibre sent over USB

- Status: Proposed
- Date: 2026-10-15

## Context
Books that Calibre's USB (MTP) Kindle driver sent are invisible to the
wireless connection, because we report only books in our `books.json`. The
result is that Calibre shows them as not on the device. Sending one wirelessly
then creates a **second copy**: our lpath template, `{title} - {authors}
({id})`, differs from the USB driver's names (for example
`Example Book - Jane Author.azw3` from USB and
`Example Book - Jane Author (42).azw3` from us). Calibre also can't delete such books, or replace
them in place, over Wi-Fi.

Evidence:
- **How Calibre matches** (`src/calibre/gui2/device.py`,
  `set_books_in_library`): a device book matches a library book by **uuid**
  first. Failing that, the title must match, plus one of `application_id`,
  `db_id`, the authors or the author sort.
- **Cache misses:** on connect we report short records (`canUseCachedMetadata`).
  Calibre asks for full metadata only for books it has no cached record for
  (`NOOP{count}`, then `NOOP{priKey}`; see
  `research/calibre-wireless-protocol.md`). An adopted book will be a miss
  the first time, so its stored record must be complete enough to send.
- **The USB driver's record** (reference device, 2026-10-15):
  `/mnt/us/metadata.calibre` (JSON list) and `driveinfo.calibre`, written by
  an MTP session on 2026-10-14 (`mtp_prefix` in driveinfo).
  - Entries for books Calibre sent have the full metadata, including `uuid`
    and `application_id`.
  - lpaths are **relative to `/mnt/us` and lowercased**
    (`documents/example book - jane author.azw3`).
  - It also lists non-books with no uuid: KOReader readmes,
    `system/version.txt`, and the Oxford dictionary under
    `documents/downloads/items01/`.
  - Four of its books are EPUBs, which the built-in reader ignores (ADR 0007).
- **The book files:** Calibre-made AZW3/MOBI files carry `calibre:<uuid>` in
  EXTH 112 and the uuid as the ASIN in EXTH 113. Both were confirmed in a
  USB-sent and a wireless-sent AZW3 on the device. Title and authors are EXTH
  503 and 100. `src/kindle.rs` already reads EXTH 113 and 501.

## Decision
**What is adopted:** a file directly under or below `documents/` that is not
yet in `books.json`, and that meets all of these:
- its extension is accepted (ADR 0007), so EPUBs and other formats are
  skipped
- its path relative to `documents/` passes `lpath::validate` (ADR 0003)
- it has a **Calibre uuid** from one of the two sources below. Books without
  one, such as Amazon purchases, dictionaries and hand-copied files, are
  never reported.

**Where the metadata comes from, in order:**
1. **`/mnt/us/metadata.calibre`.** Use the entry whose lpath, after removing
   `documents/` and comparing case-insensitively, names this file and whose
   `size` matches the file. That gives Calibre's full record. We rewrite its
   `lpath` to our form: the real filename, relative to `documents/`.
2. **The file's EXTH** (AZW3/MOBI only): the uuid from 112 (`calibre:` prefix)
   or 113, title from 503, authors from 100. Calibre fills in the rest when
   it matches by uuid.

The adopted record gets `size` and `last_modified` from the file, and our
`_kws` key the same way as received books (EXTH 113/501). If a thumbnail for
that key already exists (the USB driver writes covers too), we back it up as
we do for received books (ADR 0005). **Nothing on the device is moved,
renamed or changed.** Adoption only adds records to `books.json`, so
deleting from Calibre and replacing in place work as they do for our own
books.

**When:** automatically, at the **start of every Calibre session**, before
we answer `GET_BOOK_COUNT`. Calibre reads the book list only at that point,
so a scan during a session couldn't change what Calibre shows until the next
connect anyway. To keep it cheap, each scan `stat`s the files in
`documents/`. It reads headers only for files that aren't in `books.json`,
and it doesn't re-read a file it already rejected while that file's name,
size and mtime are unchanged. That memory is kept in RAM, not persisted.

## Alternatives considered
- **A periodic scan while connected:** rejected, because Calibre doesn't ask
  again for the book list during a session. See also the verification note
  below.
- **An "Adopt books" button in the app:** rejected (developer's choice).
  Reporting is harmless, and doing it automatically means no surprise
  duplicates.
- **Only `metadata.calibre`:** misses books that came another way, and the
  file can be stale or deleted. **Only EXTH:** loses the series, tags and
  custom columns that Calibre already recorded. Using both costs little.
- **Reporting EPUBs** so they can be deleted from Calibre: rejected. ADR 0007
  keeps formats the reader can't open out of the device's view.
- **Sharing `metadata.calibre` with the USB driver** (writing our books
  into it): out of scope. We only read it.

## Consequences
- Duplicates stop for USB-sent books. Calibre shows them on the device and
  can delete or update them over Wi-Fi.
- A USB transfer is picked up at the next session start. **To verify:** that
  Calibre won't take a USB connection while our wireless session is open
  (one device at a time), so that a USB transfer from the same Calibre always
  falls between sessions. A transfer from another computer during a session
  is picked up at the next reconnect, and Wi-Fi drops with the screensaver.
- **To verify:** that Calibre accepts a minimal EXTH-only record on a cache
  miss and matches it by uuid.
- A book from another Calibre library has a uuid our library doesn't know.
  It is adopted and shown as on the device but not in the library, which is
  accurate. Calibre deletes it only if the user asks.
- New code: a `documents/` scan, a `metadata.calibre` reader, EXTH 100/112/503
  reading, tests with fake files, and a replay case. The binary size cost
  should be small, since `serde_json` is already used, but measure it.
