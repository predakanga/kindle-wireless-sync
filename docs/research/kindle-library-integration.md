# Kindle library integration

Research for Phase 1, items 2–5: how books get into the built-in library, and
where collections, series and progress live. Sources:

- kindlewiki (github.com/sighery/kindlewiki, updated 2026-10-14):
  `kindle-hacking/cc.md`, `kindle-apps-and-services/com.lab126.scanner/*`,
  `com.lab126.ccat.md`, `com.lab126.wifid.md`, `com.lab126.powerd.md`.
- kindle-reading-dashboard (github.com/zevisvei/kindle-reading-dashboard,
  2026-08-30, tested on FW 5.18.1): `docs/cc-db-schema.md`,
  `docs/read-state-storage.md`, `docs/KRDS-format.md`.
- kindle-series-manager (github.com/mlapaglia/kindle-series-manager,
  2026-10-13): `www/cgi-bin/create.cgi`, `refresh_library.cgi`, `upload_book.cgi`.
- LibrarianSync (github.com/barsanuphe/librariansync, 2018): `cc_update.py`.
- Calibre `devices/kindle/driver.py` (USB Kindle driver).

The desk research came first. The on-device results for **FW 5.20.1**
(Paperwhite 12, probed 2026-10-15) are in the
[last section](#probe-results-fw-5201-2026-10-15). Where they disagree with
the desk research, **the probe results win**.

## Components

| Component | Role |
|---|---|
| `/var/local/cc.db` | SQLite **content catalog**. The `Entries` table holds books (`p_type='Entry:Item'`), collections (`Collection`) and series containers (`Entry:Item:Series`). The `Series` and `Collections` tables hold membership. It is held open by the catalog service. |
| `com.lab126.ccat` | Catalog service that owns `cc.db`. kindle-series-manager stops and starts it as an upstart job, but kindle-reading-dashboard reports **no such job** on 5.18.1 (the catalog lives inside `KPPMainApp`). This varies by firmware, so it needs probing. |
| `com.lab126.scanner` | **Watches `/mnt/us`** and indexes files by calling per-format *extractors* registered in `appreg.db`. Extractors build a JSON **ChangeRequest** and post it to the catalog. lipc properties: `reScanFile` (Str, w), `doFullScan` (Int, w), `triggerUpdate` (Int, w), `fullScanStatus` (Int, r). |
| `system/thumbnails/` | Library cover images, named `thumbnail_<cdeKey>_<cdeType>_portrait.jpg`. |
| `<book>.sdr/` | Per-book sidecar folder holding reader state in KRDS binary format. |
| `/mnt/us/system/fmcache/fmcache.db` | Telemetry buffer. On 5.18.1 the modern "Mark as Read" state shows up here and in the cloud, **not** in `cc.db`. |

## Getting a book into the library (requirement 4)

Candidate mechanisms, from least to most invasive:

1. **Write the file and let the scanner index it.** kindle-series-manager's
   upload and Calibre-download features only copy the file into
   `/mnt/us/documents`, which suggests the scanner picks up new files without
   help. **Still to measure:** how quickly this happens, and whether it
   happens at all while the UI is in the foreground.
2. **Ask the scanner to index one file:** `lipc-set-prop com.lab126.scanner
   reScanFile <path>`. It's a documented property with no documented
   semantics; the obvious reading is to index exactly that path. Fall back to
   `doFullScan 1` if needed. **Preferred**, because the stock extractor
   (e.g. mobi8) produces a correct entry: cdeKey/cdeType, credits, thumbnail.
3. **Post a ChangeRequest ourselves.** Two channels are known:
   - `libscanner.so`'s `scanner_post_change` (used by extractors). This needs
     an extractor shim, which is too much.
   - HTTP `POST http://127.0.0.1:9101/change` with
     `{"type":"ChangeRequest","id":1,"commands":[{"insert":{...}}|{"update":{...}}|{"delete":{"uuid":...}}]}`
     (LibrarianSync, FW ≤ 5.x 2018). **Still to check:** whether it exists on
     5.20.1. If it does, it is the cleanest way to edit *collections* and
     *series* without touching `cc.db` directly.
4. **Edit `cc.db` with sqlite** while ccat is stopped (kindle-series-manager).
   This needs the `COLLATE icu` schema workaround, which strips the collation,
   drops two indexes and restores the schema afterwards, plus a ccat restart.
   **Last resort only:** it is fragile and could corrupt the library.

Deletion has the same options. The scanner should notice when a file
disappears (an extractor receives `SCANNER_DELETE` with the entry's uuid).
`reScanFile` on a missing path might also work, which needs probing.

### Covers

The firmware shows `system/thumbnails/thumbnail_<cdeKey>_<cdeType>_portrait.jpg`.
For AZW3/MOBI, `cdeKey` and `cdeType` come from EXTH records 113 (uuid/ASIN)
and 501 (cdetype). Calibre writes these when converting. The cover images
are known to be deleted by the firmware ("Amazon cover bug"). Calibre's USB
driver handles this by writing the thumbnail itself and keeping a backup
copy to restore on later connects. We can do the same with the JPEG Calibre
sends in each book's metadata.

Calibre's conversion options set EXTH 501 to `EBOK` or `PDOC`. **To check:**
which value gives the better library behaviour on 5.20.1 (cover retention,
"Books" vs "Docs" filter).

## Collections and series (stretch 1)

Calibre can send a collections map (see `calibre-wireless-protocol.md`). There
are two possible targets on the Kindle:

- **Collections** (`p_type='Collection'` rows, plus `j_collections` /
  `j_members` and the `Collections` table). LibrarianSync created these
  through the ChangeRequest API by inserting a `Collection` and then updating
  its `members`. Since FW 5.4.2 ("cloud collections") the insert also needs
  `collectionDataSetName`, `collectionCount` and `isArchived`.
- **Native series grouping** (FW 5.13.4+): rows in the `Series` table, an
  `Entry:Item:Series` container row, and `p_seriesState=0` on member books.
  The Kindle then shows the series as one tile, like store series. This is
  probably closer to what users want from "Series → collections". kindlewiki
  lists the exact fields, and kindle-series-manager does it today with
  direct sqlite edits.

This is a product decision for later: collections, native series or both.
Both depend on the result of the ChangeRequest probe.

## Progress, read state and rating (stretch 2)

| Data | Location | Notes |
|---|---|---|
| Percent read | `cc.db` `Entries.p_percentFinished` (float 0–100) | Easy to read |
| Last position | `cc.db` `p_lastAccessedPosition`; KRDS `lpr` in `.sdr/*.azw3f` / `*.azw3r` | |
| Last access | `cc.db` `p_lastAccess` (epoch seconds) | Could map to Calibre's `_last_read_date_` |
| Read state | Legacy `cc.db` `p_readState` (0/1/2). Modern "Mark as Read" is in `fmcache.db` (`records` where `schema_name='mar_content_readstate_update_success'`) and the cloud | fmcache is a rolling buffer that gets cleared after upload, so it's unreliable for an offline device |
| Rating | Unknown. The built-in reader may not store a per-book rating for sideloaded books at all | Needs probing |

Stock Calibre can receive `_is_read_` and `_last_read_date_`. Percent read and
rating need the Calibre plugin.

## Network and power (for ADR 0002)

- `com.lab126.wifid` has the property `cmState` (e.g. `CONNECTED`).
  Connection changes are expected to fire lipc events on `wifid` or
  `com.lab126.cmd`. **Still to identify** with `lipc-wait-event -m`.
- `com.lab126.cmd` `ensureConnection` (w) presumably asks the system to bring
  Wi-Fi up.
- `com.lab126.powerd` events `goingToScreenSaver`, `readyToSuspend` and
  `outOfScreenSaver` drive suspend handling (as in HID passthrough).
  `deferSuspend` (w) might keep the device awake during a transfer.

## On-device probes needed (FW 5.20.1)

Each can be run over SSH. Results go back into this doc.

1. `cat /etc/prettyversion.txt`, `uname -a`, and how KPM reports the
   platform (`kindlehf`?).
2. `initctl list | grep -i -e ccat -e scanner`. Is ccat an upstart job?
3. `netstat -ltnp` / `ss -ltnp`. Is anything listening on `127.0.0.1:9101`?
4. Copy a Calibre-converted AZW3 into `/mnt/us/documents` and time how long
   until it appears in the library, with no other action. Then the same with
   `lipc-set-prop com.lab126.scanner reScanFile /mnt/us/documents/<file>`.
5. Remove that file and check whether the entry goes away, with and without
   `reScanFile`.
6. `sqlite3 /var/local/cc.db '.schema Entries'` and a dump of the new book's
   row (cdeKey, cdeType, thumbnail, location).
7. `lipc-wait-event -m com.lab126.wifid '*'` and `com.lab126.cmd '*'` while
   toggling airplane mode, to find the network-up event.
8. Check whether a `.sdr` sidecar holds any rating, and which `cc.db` fields
   change after reading a few pages, finishing a book and marking it read.

## Probe results (FW 5.20.1, 2026-10-15)

Probed over SSH (usbnet) on the reference device. Test files were named
`kws-*` and removed afterwards, together with the `.sdr` folders the indexer
created for them.

### Platform

- `Kindle 5.20.1`, kernel `5.15.41-lab126`, `armv7l`, hard-float loader
  (`/lib/ld-linux-armhf.so.3`). **KPM platform `kindlehf`**: KPM keeps
  `/var/local/kmc/kindlehf`, and installed packages such as kterm list only
  `kindlehf`.
- There is **no `com.lab126.ccat` upstart job** (matching the 5.18.1 report).
  The jobs that are present include `scanner`, `kppmainapp`, `framework`,
  `cmd`, `wifid` and `appmgrd`. kindle-series-manager's
  `stop/start com.lab126.ccat` **would not work** on this firmware.
- **`127.0.0.1:9101` is listening**, owned by the Java framework process. So
  the ChangeRequest endpoint likely still exists. We haven't sent it anything
  yet, because the base requirements don't need it.

### Two library stores

5.20.x has a **second catalog**, `/var/local/ksdk.asset.db`, which
kindlewiki doesn't document. It has the tables `Nodes` (assets), `Tags`
(`SERIES`, `USER_DEFINED_TAG` = collections, `SYSTEM_TAG` = Favorites) and
`ContentTag` (membership). Its key is `ASSET_ID = <cdeKey>!!<cdeType>`.
`KPPMainApp`, the new library UI, is the presumed reader of it.

`cc.db` is still kept up to date and feeds the asset DB. Triggers on
`Entries`, `Collections` and `Series` write to `CatalogChanges` and then
`AuditLog` (`changeType` LIBRARY_ITEM / COLLECTION / COLLECTION_RELATION /
SERIES / SERIES_RELATION; `requestType` DOUBLE_WRITE / BACKFILL /
MERGE_CHANGE), and `MigrationConfig.migrationVersion = 4`. Only the shell
scriptlets (`text/x-shellscript`) show `FAILED` in `AuditLog`. Books relay
fine.

Consequence: anything that edits the library by hand has to keep **both**
stores consistent. We should only go through the system's own paths (the
scanner, or possibly the ChangeRequest API) and never edit either database
directly.

### Indexing

| Test | Result |
|---|---|
| Write a new `.txt` into `documents/` | In **both** `cc.db` and the asset DB within **~2 s**, with no trigger. `PDOC`, `cdeKey = "*" + sha1`, `displayTags ["NEW"]` |
| Write slowly (two chunks 6 s apart) | Not indexed until the file was **closed**; the entry showed the final size |
| Write a dot-prefixed temp name (`.x.part`) | Not indexed |
| `mv` the temp file to a new `.txt` name | Indexed within ~2 s |
| `mv -f` a new file **over** an existing indexed file | **Entry not updated** (size stayed stale) |
| Rewrite an existing file in place | Entry updated; `p_uuid` kept |
| Delete the file | Removed from both stores within ~2 s |
| `lipc-set-prop com.lab126.scanner reScanFile <path>` | **Fails**: `lipcErrInternal` (0x2) |
| Concurrent read of `cc.db` | Occasionally `database is locked` |
| Existing EPUBs in `documents/` | **Not in the catalog at all.** The built-in reader ignores sideloaded EPUB |

The indexer also creates `<name>.sdr/` with a small `.mbp1` for every book
and **leaves it behind** when the book is deleted. Sidecar files are named
`<basename><32-hex>.<ext>`. The hex suffix was the same for every book
(`d3efa7f7…`), so it's a per-device or per-account constant, not a hash of
the path.

**So requirement 4 is met by the scanner alone:** a book written into
`documents/` appears in the library within seconds.

#### AZW3 (mobi8 extractor)

Two books from the Calibre library were converted with `ebook-convert`
(Calibre 9.16.0) and uploaded with the ADR 0005 pattern: a dot-prefixed temp
file, then `mv` to the final name.

- Indexed into **both** stores in **~2 s**: `EBOK`,
  `application/x-mobi8-ebook`, with title and credits taken from the file.
  `cdeKey` = the book's EXTH 113 value, and `ASSET_ID = <cdeKey>!!EBOK`. It
  appeared at the top of the Library with the **"New"** badge.
- The extractor writes `system/thumbnails/thumbnail_<cdeKey>_EBOK_portrait.jpg`
  itself, but it's a **60×40 "No image available" placeholder** (961 bytes).
  With no other cover, the Library shows a **grey generic tile** with the
  title and author (confirmed by screenshot).
- Overwriting that file with Calibre's cover (331×500 JPEG, ~41 KB) **was not
  undone** within 60 s. The Library showed the real cover **the next time it
  drew the grid**; it does not update an already-drawn tile. So the cover
  must be written **before** the book is renamed into place.
- The `.sdr/` folder is created lazily (for one book it only appeared once
  the Library had drawn it), and it's still left behind after deletion.
- Standalone `ebook-convert` takes the EPUB's own embedded metadata, so the
  title was "Artificial Condition--The Murderbot Diaries" and EXTH 113 was a
  **new random UUID** (not the library UUID `673695bc…`). When Calibre sends
  to a device it writes the library metadata into the file first, so the
  client should expect EXTH 113 to be the Calibre book UUID. **Still to
  check** once the client exists.

### Covers

The existing Calibre-sent AZW3 (via USB) has `cdeKey` = Calibre's book UUID
and `cdeType = EBOK`. Its thumbnail is at
`system/thumbnails/thumbnail_<uuid>_EBOK_portrait.jpg` (written by Calibre's
USB driver), with a backup in `/mnt/us/amazon-cover-bug/`. The asset DB's
`Nodes.THUMBNAIL` points at the same path.

#### Cover written before the book appears (2026-10-15)

A third converted AZW3 (`cdeKey` `cb5d2673…`, freshly random so nothing could
be cached) had its 54,665-byte cover written to
`system/thumbnails/thumbnail_<cdeKey>_EBOK_portrait.jpg` **before** the book
was renamed into `documents/`. The extractor indexed the book in ~2 s and
**left the existing thumbnail alone**. The file was unchanged after 15 s,
`p_thumbnail` pointed at it, and the Library showed the real cover the first
time it drew the book. Not covered: a Library already on screen during the
sync, which we know only redraws on refresh.

#### `cdeKey` for files without their own key

For the `.txt` probe and every `.sh` scriptlet on the device,
`p_cdeKey = "*" + sha1_hex(absolute path)`. For example,
`sha1("/mnt/us/documents/BTManager.sh")` is `74be0406…`, giving
`*74be0406…`. The `cdeType` was `PDOC`. Books with EXTH 113 use that value
instead (see above).

#### PDF (2026-10-15)

A one-page PDF got `cdeKey` `"*" + sha1(path)`, `PDOC`, `application/pdf`,
but **no thumbnail**: `p_thumbnail` was `0`, the asset DB `THUMBNAIL` was
empty, and no thumbnail file was created. The indexer did create a `.sdr/`
folder with render-cache subdirectories. The Library showed a generic tile
with a PDF icon and the title. Writing `thumbnail_<cdeKey>_PDOC_portrait.jpg`
by hand **had no effect** after a redraw. So covers can only be supplied for
EXTH-bearing formats (AZW3/MOBI/AZW) and probably KFX. PDF and TXT keep
generic tiles.

#### End-to-end on the device (2026-10-15)

The client ran on the Kindle (FW 5.20.1) against the developer's Calibre
9.16.0 with covers enabled. Calibre sent an EPUB-only book (converted to
AZW3) and a MOBI:

- Both were indexed in `cc.db` and the asset DB as `EBOK` with
  **`cdeKey` = the Calibre book UUID**. The MOBI went through the mobi (not
  mobi8) extractor and behaved the same.
- With the **Library already on screen**, both tiles appeared live with
  their **real covers** and the "New" badge. The Library adds new tiles when
  the catalog changes, and our covers were already in place.
- **Re-sending** a book rewrote it in place: same `p_uuid`, and the `.sdr`
  was kept.
- **Deleting** from Calibre removed the file, `.sdr`, both catalog entries,
  the thumbnail and our cover backup.
- When the device went to the **screensaver, Wi-Fi went down** (`cmState NA`,
  no default route) and a session started then failed with
  "could not send any discovery probe". The daemon must wait for
  Wi-Fi (ADR 0004).

### Indexing time and the catalog's event (2026-10-18)
Measured by copying a file into `documents` (temp name, then `mv`) and
reading `cc.db` every second:
- A KFX copy was listed after 14.7 s, an AZW3 copy after 14.5 s. The
  scanner works through new files one at a time, so books arriving about
  10 s apart queue up.
- `com.lab126.ccat` sends an `updated` event when the catalog changes:
  12.8 s after the file arrived, about when the entry appeared, and again
  3 s later. `com.lab126.scanner`, `com.lab126.indexer`,
  `com.lab126.kpp.yjextractor` and `com.lab126.extractor.java` sent nothing
  (`lipc-wait-event -m <service> '*'`). Nothing was logged in
  `/var/log/messages` either.
- `cc.db` is a single SQLite file (no `-wal`), so inotify on it in
  `/var/local` would also see the change; the event is simpler.
- A KFX copy of a book already in the library got a key made from its path
  (`*58026a…`) instead of the original's ASIN.

### Deleting through the catalog (2026-10-18)
- **The scanner is slow to remove entries.** Four files deleted at once:
  the first entry went after 16.7 s, then one every 3 to 4 s (27.3 s for
  all four).
- **ChangeRequest `{"delete": {"uuid": <p_uuid>}}`** removes a downloaded
  entry from `cc.db`, and the asset store's `Nodes` row goes with it. The
  handler (`/usr/lib/ccat/change.lua`, `construct_delete_sql`) has a path
  for exactly this ("Deletion of downloaded entry"). Tested with TXT files:
  deleting the entry first or the file first both worked, the entry was
  gone within 0.2 s, and the scanner didn't re-add it.
- **The catalog can be locked** while the scanner works: the change then
  fails with HTTP 503 and `"sqlite3_msg":"database is locked"`. Retrying a
  second later works.
- **The Library follows catalog deletes** within a couple of seconds,
  without being redrawn.
- Don't test with copies of real books: a copy shares the original's EXTH
  key, and so its asset store row (`ASSET_ID = <cdeKey>!!<cdeType>`).

### Verifying the UI from SSH

`/usr/sbin/screenshot` (stock) writes a PNG of the framebuffer to
`/mnt/us/screenshots/`. Kindle Button Mapper's `scripts/tap.sh X% Y%` injects
a tap, for example `76 96` for the Library tab on the reference device.
`lipc-set-prop com.lab126.appmgrd start app://com.lab126.booklet.home` goes to
Home.

### Network-up events

Captured while toggling `lipc-set-prop com.lab126.cmd wirelessEnable 0/1`
(Wi-Fi was back to CONNECTED within 3 s):

- `com.lab126.wifid`: `cmStateChange "NA" | "READY" | "PENDING" | "CONNECTED"`,
  `cmConnected`, `cmDisconnected`.
- `com.lab126.cmd`: `connectionAvailable "wifi" "internet"` (fired 4 times),
  `InternetConnected 1`, `connectionNotAvailable`, `wirelessEnableChanged 0|1`.

Use `wifid cmStateChange "CONNECTED"` as the network-up trigger, since it
doesn't depend on internet access, and debounce it. Use
`cmStateChange "NA"` / `cmDisconnected` for network-down. Current state can
be read from `lipc-get-prop com.lab126.wifid cmState`.

### Progress and rating

- `cc.db`: `p_percentFinished` (e.g. 2.4585876), `p_lastAccess` /
  `p_lastOpenTime` (epoch s). `p_lastAccessedPosition` was empty. The asset
  DB mirrors the percentage as `Nodes.PERCENT_READ` (generated from
  `ADDITIONAL_DATA.percent_read`) and has `READ_STATE`.
- Sidecars (KRDS-decoded): `.azw3f` holds `lpr`/`fpr` positions,
  `timer.model` and `book.info.store`. `.azw3r` holds `font.prefs`,
  `StartActions`, `sync_lpr`, `next.in.series.info.data` and `apnx.key`.
  **There's no rating anywhere.** The built-in reader doesn't seem to store a
  per-book rating for sideloaded books, so **rating sync is probably not
  possible** unless something writes a rating after the end-of-book flow.
  Not tested. **Correction (2026-10-16):** the developer points out that
  the Kindle prompts for a Goodreads rating at the end of a book, so a
  rating may be stored once a book has been rated. No book had been rated
  when this was probed. See "Read state probe" below.

### Read state probe (2026-10-16)

For ADR 0017. A sideloaded AZW3 (`kws-read-probe.azw3`, made with
`ebook-convert`, with the service stopped so it wasn't adopted) was
driven with `tap.sh` and screenshots. `cc.db`, the asset store,
`fmcache.db` and the `.sdr` folder were compared after each step.

| Step | `cc.db` `p_readState` | asset `READ_STATE` | Other changes |
|---|---|---|---|
| Indexed (never touched) | 0 | 0 | `p_lastAccess` = index time, `p_lastOpenTime` unset |
| Library ⋮ → "Mark as read" | **1** | 1 | `read_state_origin` 1, fmcache `mar_content_readstate_update_success` (`MANUAL`, `DETAIL_VIEW`) |
| ⋮ → "Mark as unread" | **3** | 2 | fmcache `previous_read_state: READ_MANUAL`, `read_state: UNREAD` |
| Opened | 3 | 2 | `p_lastOpenTime` and `p_lastAccess` set to the open time, `.sdr` created |
| Turned past the last page | **2** | 1 | `read_state_origin` 2, fmcache `mar_auto_markasread` (`BOOK_END_POSITION`) and `AUTOMATIC`/`END_OF_BOOK` |
| Closed (back to Library) | 2 | 1 | `p_lastOpenTime` and `p_lastAccess` set to the close time, `p_percentFinished` 98.9, asset `PERCENT_READ` too |

- **On 5.20.1 the read state is in `cc.db`**, unlike the 5.18.1 report
  above (fmcache only). `p_readState`: 0 = never set, 1 = marked read,
  2 = marked read automatically (end of book), 3 = marked unread. The
  asset store uses 0, 1 (read), 2 (unread).
- **`p_lastOpenTime` is the "last read" time:** unset until the book is
  first opened, and set on open and on close. `p_lastAccess` is also set
  when the file is indexed, so it isn't a reading signal.
- **`p_lastOpenTime` arrived in 5.19.1** (2026-10-20). The catalog's
  schema migrations are plain SQL in `/usr/share/cc/cc.sqls`
  (`--## Update: Entries:N:M` blocks), so each firmware's latest
  `Entries` version can be read from its `rootfs.img` without booting it
  (method: "Catalog support across firmware", below):

  | Firmware | Last `Entries` migration |
  |---|---|
  | 5.17.1.0.2 (Paperwhite 6) | 78: `p_subType` |
  | 5.18.6 (Paperwhite 6) | 78 (79 is metadata only) |
  | 5.19.1 (Scribe 3) | 81: `p_purchaseDate` (80), **`p_lastOpenTime` (81)** |
  | 5.19.2 (Paperwhite 6) | 82: `p_conversionStatus` |
  | 5.20.1 (Paperwhite 6) | 86: `p_embeddedCdeKey` |

  In the emulator's release lists, 5.19.1 exists only for the Scribe 3
  and Scribe Colorsoft. The Paperwhite 5 and 6, Basic 2024, Colorsoft and
  Scribe 2 went from 5.18.6 to 5.19.2, so for them 5.19.2 is the first
  firmware with the column. 5.18.6 has the name only in a binary's
  string table, not in its schema. Compat runs agree: 5.17.1.0.2
  (Paperwhite 6) and 5.16.2.1.1 (Basic 2016) lack it; 5.19.2 and 5.20.1
  have it. Read sync's query names the column, so it fails on older
  firmware (not yet handled).
- **No end-of-book screen** appeared for this sideloaded book (no "Before
  you go", no Goodreads rating prompt). The read state changed silently.
  The developer then checked by hand, reaching the end each time with the
  progress slider and then paging forward:

  | Book | Device | "Before you go" |
  |---|---|---|
  | Sideloaded earlier with this app | reference: Paperwhite 12th gen (5.20.1) | no |
  | Amazon book | reference (5.20.1) | **yes** |
  | Sideloaded by Calibre (USB) | Paperwhite 10th gen (5.17.1) | **yes** |
  | The same book, sent by Calibre over USB (MTP) | reference (5.20.1) | no |

  So on 5.20.1 the end-of-book page, and the Goodreads rating it asks for,
  only appear for Amazon books, whatever the transfer method. **Rating sync
  is out of scope** (developer, 2026-10-16; ADR 0017).
- **ChangeRequest can set it:** `{"update": {"uuid": <p_uuid>,
  "readState": 3}}` returned `ok`, set `p_readState` 3 and asset
  `READ_STATE` 2 (through the triggers), and the Library tile changed from
  the "Read" banner to "99%" at once, with no refresh. `readState: 1` turned
  it back to read, with the banner shown.
- Cleanup: the book, its `.sdr` folder and its thumbnail
  (`thumbnail_<cdeKey>_EBOK_portrait.jpg`, written by the firmware) were
  removed, and the catalog entry went with the file.

## Native series on FW 5.20.1 (2026-10-16)

Phase 5 research: showing Calibre series as the Kindle's own series grouping
(one tile per series, in order). In short: **we can write correct series
data through a supported interface**, but the Kindle only displays it where
series grouping is on, and Amazon offers that only in some marketplaces
(see the end of this section).

### The catalog's ChangeRequest endpoint

- `POST http://127.0.0.1:9101/change`, with the header
  `AuthToken: <contents of /tmp/session_token>` (44 bytes, rotated at
  least on every reboot). Without the token the reply is
  `401 {"Error": "Unauthorized request."}`. This requirement dates from
  about FW 5.12 (MobileRead LibrarianSync thread), and LibrarianSync was
  reported working with it on 5.16.2.1.
- The body is `{"type":"ChangeRequest","id":N,"commands":[...]}`, and the reply is
  `{"ok":true,"type":"ChangeResponse","seriesProcessed":0,"id":N,"changes":K}`.
- The handler is plain Lua on the device: `/usr/lib/ccat/change.lua`,
  `series.lua` and `sql_functions.lua`. It's Amazon's code, so read it on
  the device and don't copy it into the repo.
- Command shapes, read from that code and confirmed on the device:
  - `{"update": {"uuid": <p_uuid>, <field>: <value>, ...}}` updates fields of
    an `Entries` row. Unknown field names are rejected. The accepted fields
    include `seriesState`, `isProcessed`, `titles`, `credits`, `cdeGroup`,
    `homeMemberCount` and `virtualCollectionCount` (but not `memberCount`).
  - `{"insertOr": {"onConflict": "REPLACE", "entry": {...}}}` inserts an
    `Entries` row. For `Entry:Item:Series` it matches an existing row by
    `cdeGroup`. Inserts force `seriesState = 1`, and for series containers
    `isProcessed = 0`.
  - `{"insertOr": {"table": "SERIES", "onConflict": "REPLACE", "entry":
    {"seriesId", "itemCdeKey", "itemType", "itemPosition",
    "itemPositionLabel", "seriesOrderType"}}}` writes a `Series` row.
  - `{"delete": {"table": "SERIES", "seriesId": ..., "itemCdeKey": ...}}`
    deletes one, and `{"delete": {"uuid": ...}}` deletes an `Entries` row.
  - Collections (deferred, see the plan's Later list): the Library app's
    own code sends `addCollectionMember` / `removeCollectionMember` with
    `collectionUuid`, `memberCdeKey` and `memberCdeType`.
- Changes reach the second store, `ksdk.asset.db`, through the catalog's
  triggers. Series containers become `Tags` rows (`<key>!!series`,
  `TAG_TYPE = SERIES`), and memberships become `ContentTag` rows. Series
  aren't `Nodes`.

### What a series needs

Following kindle-series-manager's working rows (github.com/mlapaglia/
kindle-series-manager, `kindle_series.py`, which writes them with SQL; a
user reports it working on 5.19.2), but sent through ChangeRequest:

1. A `Series` row for each book, with `seriesId =
   urn:collection:1:asin-<KEY>`, the book's `cdeKey`, its position (0-based)
   and label, `itemType = Entry:Item` and `seriesOrderType = ordered`.
2. An `Entry:Item:Series` container: `cdeKey = <KEY>`, `cdeType = series`,
   `cdeGroup = <seriesId>`, titles, credits, `mimeType =
   application/x-kindle-series`, `isArchived = 1`, `originType = -1`, and
   `homeMemberCount`.
3. `update` the container to `isProcessed = 1`, and each member book to
   `seriesState = 0`.

**`<KEY>` must be exactly 10 characters**, like an ASIN. The trigger that
copies a `Series` row to the asset store uses the last 10 characters of
`seriesId` as the series id (`get_series_relation_external_id`). With an
8-character key, the container reached the asset store but the members
didn't. With `KWSTEST001`, both did.

**The firmware's own post-processing** (`postProcessSeries`) hides a
series's books only when the series has at least 2 *cloud* copies
(`p_isArchived = 1`). Otherwise it sets them back to visible. Sideloaded
books have no cloud copy, so it would undo step 3 whenever it runs for that
series. That happens on device-side inserts and deletes of a member book,
and when it processes containers with `isProcessed` not 1. A sync would
have to re-apply step 3 after any change.

The new Library app works out grouping itself in the asset store:
`Nodes.VISIBILITY_WITH_GROUPING = 1` means "member of a series with 2 or
more books". After the steps above, both test books had it set to 1, as a
real 4-book Amazon series does.

### Grouping is gated per marketplace

On the reference device (AU marketplace) even Amazon series weren't
grouped at first, and there was no switch for it: Amazon offers series
grouping only in some marketplaces, through its remote config. Series data
written as above shows only where grouping is on.

Turning it on where it isn't offered changes Amazon's settings, so it's
not part of this package. That research moved to
[Kindle Series Enabler](https://github.com/predakanga/kindle-series-enabler)
on 2026-10-20. The reference device has grouping on.

### Making a sideloaded series show (2026-10-16)

With grouping switched on (see above), the test series's
books were hidden but there was no series tile, even after a reboot. The
cause is the **Library's legacy listing filter** on `cc.db` (strings in
`KPPMainApp`; this device has no library re-architecture). Roughly:

- *All*: hide books with `p_seriesState` 0 or 2, and hide series containers
  with `p_memberCount <= 1`.
- *Downloaded*: hide books with `p_seriesState` 0, and hide containers with
  `p_homeMemberCount <= 1`.

The firmware's post-processing (`updateSeriesMetaData`) sets
`p_memberCount` to the number of **cloud** copies and `p_homeMemberCount`
to the number of downloaded ones. So a sideloaded series gets
`p_memberCount = 0`, and its tile never shows under *All*. `memberCount`
isn't an accepted ChangeRequest field.

**Fix, through ChangeRequest:** `update` the container with
`"members": [<member Entries uuids>]` (plus `type` and `cdeType`). The
catalog's `members` handler writes `Collections` rows linking container
and books, and sets the container's `p_memberCount` **and**
`p_homeMemberCount` to their count (2 and 2 here). The member books'
`p_collectionCount` and `j_collections` stayed unchanged (0 and `[]`),
because those only count `Collection` entries. With that, the **tile
appeared** (stacked, "2" badge). Opening it showed both books in series
order, labelled 1 and 2. `"members": []` undoes it.

**Kindle Unlimited badge:** the tile showed a "kindleunlimited" badge while
the member books had `p_originType` NULL (as sideloaded books do). Amazon
series members have `0` (purchased). `update` members to `originType: 0`
removed the badge. The same NULL books show no badge on their own tiles.
Reversible with `"originType": "null"`. Whether 0 has other effects on
sideloaded books is untested.

**Other details:**
- The series view shows "You have N out of M items in this series", where M
  comes from the container's `virtualCollectionCount`. kindle-series-manager
  writes member count + 1, so it showed "2 out of 3". Use the real size.
- A series cover needs a real thumbnail. The test books were copied by hand,
  so they only had the scanner's 961-byte placeholder (grey tile). Books
  from our client get real covers.
- `homeMemberCount` sent in the container insert was dropped (it ended up
  0), but `members` sets it.

**What normal sync activity does to it** (tested 2026-10-16):
- **Replacing a member file in place** (our client's update path, ADR
  0005): the scanner re-indexed it, and the series stayed intact (counts,
  `seriesState`, links and `Series` rows). But **re-indexing reset that
  book's `originType` to NULL**, which brings the badge back, so re-apply
  `originType: 0` after re-indexing a member.
- **Deleting a member file:** the firmware's post-processing ran and
  degraded the series safely. It reset the counts to (cloud 0, downloaded
  1), made the remaining book visible (`seriesState 1`, and
  `VISIBILITY_WITH_GROUPING 0` in the asset store), and removed the deleted
  book's link row and asset-store membership. It **left the deleted book's
  `Series` row behind** (orphaned). After a delete, the sync must remove
  that row, and either re-apply the series (2 or more members left) or
  dissolve it (fewer).
- **Cleanup** through ChangeRequest worked: `members: []`, delete the
  `Series` rows, delete the container. Both stores were then clean. The
  scanner's placeholder thumbnails for the deleted test books stayed behind
  and were removed by hand.

**Still to check before building on it:**
- Whether it survives a reboot with the counts set (the earlier reboot was
  before `members`).
- ~~Whether anything tries to sync the series or the `Collections` rows to
  Amazon~~: no sign of it (ADR 0016, "Device test"). Every tag reads
  `PENDING_SYNC`, the account's own collections included, while the
  membership rows (`ContentTag`) read `SYNCED`, ours and Amazon's alike.

### Test data (removed 2026-10-16)

The test series and both test books were removed on 2026-10-16. The
details below are kept for reference.

- Files `kws-series-1.mobi` (cdeKey `e1dddd39-…`, `Entries` uuid
  `4f6374f3-962a-4f6d-9260-4146b47460af`) and `kws-series-2.mobi` (cdeKey
  `1cc30fc7-…`, uuid `fb8f3784-6416-4819-83d2-da82f836aec3`) in
  `/mnt/us/documents`, with their `.sdr` folders.
- The series `urn:collection:1:asin-KWSTEST001` ("kws-test series"):
  container `Entries` uuid `40757f9c-53ee-4a9c-8167-9aa1c3827a27`
  (`p_cdeKey = KWSTEST001`), both books at positions 0 and 1, with
  `seriesState = 0`.
- **To clean up**, through ChangeRequest: delete both `Series` rows (`table:
  SERIES`), delete the container by uuid, set both books' `seriesState` back
  to 1 (or just delete the files, after which the scanner removes the
  entries), then remove the files and `.sdr` folders. Until the series rows
  are gone, the two books are hidden only if grouping is on, so they stay
  visible on this device.

## Books in subfolders are indexed (2026-10-18)
For PLAN Later item 12 (choosing where books are stored). On the reference
Kindle (5.20.1), one TXT was written to `/mnt/us/documents/` and another to
`/mnt/us/documents/kws-subfolder-probe/`. Within 40 s both were in `cc.db`
with identical flags (`p_visibilityState = 1`, `p_isVisibleInHome = 1`,
`p_cdeType = PDOC`, `p_isArchived = 0`). Only the identifying fields
differed (`p_uuid`, `p_location`, titles, sizes, `p_cdeKey`). So the
scanner indexes subfolders of `documents` like `documents` itself; the
Library just doesn't show folders. The indexer made each book's `.sdr`
folder next to the book, inside the subfolder too. Deleting the files
removed both entries within 30 s.

## Fractional series positions (2026-10-18)
For PLAN Later item 13. The developer sent five Mistborn books from Calibre,
including *Mistborn: Secret History* at `series_index` 3.5. The series pass
stored it as `d_itemPosition` 3.5 (`typeof` real; the column is a double)
with `d_itemPositionLabel` "3.5", and the series view ordered it between 3
and 4. But its badge showed "3". Setting the label to "Z" through
ChangeRequest made the badge show "0", so **the Library parses the label as
a whole number for the badge**, and no label can show a fraction there. The
label was then set back to "3.5". A series pass runs only when something
schedules one, so it didn't notice the test label.

## Catalog support for series across firmware (2026-10-20)
Which kindlehf firmware (5.16.3 and later; 5.16.2.1.1 and older are
soft-float) has what the series pass (ADR 0016) needs. Checked without
booting, from the root filesystems of the emulator's firmware (eink-emulator
`firmware/versions/`), with Homebrew's e2fsprogs `debugfs`:
`debugfs -R "dump /usr/share/cc/cc.sqls OUT" rootfs.img` and `rdump
/usr/lib/ccat`. Don't search `rootfs.img` directly: the files are split
across blocks, and a raw scan reads into unrelated data (it showed
`j_members` missing on 5.13.6 to 5.18.6, which was wrong).

| Firmware (models) | `cc.sqls` | `ccat/change.lua` |
|---|---|---|
| 5.16.3 (Basic 2022, Paperwhite 5) | A | 5.16.3 |
| 5.17.1.0.2 (Basic 2024, Paperwhite 6) | A | 5.17.1.0.2 |
| 5.17.3 (Scribe 2), 5.18.1 (Colorsoft) | A | 5.17.3 |
| 5.18.1.1.1 (Paperwhite 4) | A | 5.18.1.1.1 |
| 5.18.6 (Paperwhite 6) | A | 5.18.6 |
| 5.19.2 (Paperwhite 6) | B: A + `Entries` 80 to 82 | 5.19.2 |
| 5.20.1 (Paperwhite 6, Basic 2024, Colorsoft, Scribe 1 and 2) | C: B + audit log | 5.20.1 |

A is byte-identical from 5.13.6 to 5.18.6 (`Entries` 79, `Series` 5).

- **Every column the series pass reads or writes is there on all of
  them.** `Entries`: `p_uuid`, `p_location`, `p_cdeKey`, `p_cdeType`,
  `p_type`, `p_thumbnail`, `p_seriesState`, `p_originType`,
  `p_titles_0_nominal`, `p_memberCount`, `p_homeMemberCount`,
  `p_isProcessed`, `p_virtualCollectionCount`, `j_members` (read), and
  `p_cdeGroup`, `j_titles`, `j_credits`, `j_displayObjects`, `p_mimeType`,
  `p_isArchived`, `p_isVisibleInHome`, `p_isLatestItem` (set by our
  commands). `Series`: all six columns, including `d_itemType` and
  `d_seriesOrderType` (`Series` 5; 5.12.2.2 had version 3, without
  `d_seriesOrderType`).
- **The ChangeRequest handler takes every command we send** on all of
  them. `change.lua`'s `column_specs` maps every field our commands use
  (64 fields on 5.16.3 to 5.18.6, 68 on 5.20.1), and it has
  `insertOr_series` (`INSERT OR REPLACE` and `INSERT OR IGNORE INTO
  Series`) and `delete_series`. It keeps `p_memberCount` and
  `p_homeMemberCount` itself; `cc.sqls` has no triggers for them. The
  handler changed in almost every release, so behaviour can still differ.
- **The endpoint** (`127.0.0.1:9101/change`) is served by the Java
  framework, so it can't be checked statically. The 5.17.1.0.2 compat run
  (Paperwhite 6, 2026-10-19) passed `catalog_endpoint` (401 without the
  token) and `series`, which sends the same command shapes as
  `src/series.rs`: it made a two-book series and dissolved it.
- **Grouping in the Library:** `series.grouping.supported`, the setting
  Kindle Series Enabler overrides, is in every one of these images. The
  series view itself has only been seen on 5.20.1.
- The compat check `catalog_schema` now covers the columns our commands
  set as well as those `src/` reads. The reference device's live `cc.db`
  (5.20.1) has all ten written columns (developer's run, 2026-10-20).
