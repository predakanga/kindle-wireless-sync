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
  Not tested.

## Native series on FW 5.20.1 (2026-10-16)

Phase 5 research: showing Calibre series as the Kindle's own series grouping
(one tile per series, in order). In short: **we can write correct series
data through a supported interface, but this device doesn't display series
grouping at all**, because the feature is gated per marketplace (see the end
of this section).

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

### Why nothing showed: grouping is gated

On the reference device even Amazon series (for example a 4-book one) are
**not grouped**, and there's no switch for it. From `KPPMainApp`'s strings:

- The switch is a **Quick Settings** tile (`group_series_library`,
  `QuickSettings::GroupSeriesWidgetController`), and it isn't offered on this
  device.
- Availability depends on remote config (`series.grouping.supported`,
  `series.grouping.supported.marketplaces`) and a weblab,
  `BOOKS_EINK_SAGA_LSG_431259` (`isSeriesGroupingWeblabEnabled`). This
  device is on the **AU** marketplace.
- The user's choice was moved from dynamic config (`series.grouping.enabled`,
  `IGNITE_SERIES_GROUPING_ENABLED_KEY`) into "UserDefaults" (marker file
  `/var/local/SERIES_GROUPING_MIGRATION_DONE`, 2026-10-13). On this device
  `/var/local/user_defaults.db` (table `user_defaults`, columns
  `ID|Key|Value`) has no series-grouping key.
- Weblab state is in `/var/local/weblab/` (`allocations/*.alloc`, a large
  read-only key/value DB, plus `overrides/` and `local_overrides/` folders,
  and `/var/local/weblab_overrides`). Their formats haven't been looked at.

Enabling grouping on one's own device is a separate, personal investigation
(started 2026-10-16), not part of the package: the app mustn't change
Amazon's settings or experiment flags.

### Enabling grouping on the reference device (2026-10-16)

Read from `KPPMainApp`'s `KPPSettings::ServiceProvider::GroupSeriesServiceProvider`
(disassembled; it's Amazon's code, so nothing copied here):

- Every check first asks `IgniteUtils::IsLibraryRearchitectureEnabled()`
  (the Ignite "library rearch" module). If that's true, the switch is
  always offered and its value lives in **UserDefaults**. Otherwise:
  - `shouldWidgetBeDisplayed` = dynconfig `series.grouping.supported ==
    "true"` **and** `series.grouping.supported.marketplaces` contains
    dynconfig `marketplace.obfuscated.id` (a substring match).
  - `getGroupBySeriesConfig` = dynconfig `series.grouping.enabled == "true"`.
  - `setGroupBySeriesConfig` writes both dynconfig and UserDefaults.
  - `isSeriesGroupingWeblabEnabled` (`BOOKS_EINK_SAGA_LSG_431259 == T1`)
    has no direct callers, so the weblab doesn't seem to gate it.
- **Dynconfig** (`libdynconf`) lives in `/var/local/appreg.db`, table
  `properties`: handler `dcd` holds the defaults and `dcc` the current
  (server-written) values, which win. On this device: `dcd` supported =
  `true`, marketplaces = `A1VC38T7YXB528,ATVPDKIKX0DER,A1F83G8C2ARO7P,A1PA6795UKMFR9`
  (JP, US, UK, DE); `dcc` `marketplace.obfuscated.id = A39IBJ37TRP1C6`
  (AU) and `dcc` `series.grouping.enabled = false`.
- **UserDefaults** (`libEReaderAppSDKShared.so`, `esdk::UserDefaultsImpl`)
  stores rows in `/var/local/user_defaults.db` table `user_defaults` with
  `Key = <namespace>.<key>`. The grouping flag is
  `IGNITE_LIBRARY_NAMESPACE.IGNITE_SERIES_GROUPING_ENABLED_KEY`. Existing
  rows use `ID = 1` and bools are stored as `1`/`0`.

What was changed (by hand, with sqlite3; backup of `appreg.db` from before
at `/mnt/us/kws-appreg.db.bak`):

1. Inserted `dcc | series.grouping.supported.marketplaces |` the `dcd` list
   plus `A39IBJ37TRP1C6`. Quick Settings still showed no tile, even after
   `initctl restart kppmainapp` (that restart also popped an "Application
   Error" dialog from appmgrd, which was harmless).
2. Set `dcc series.grouping.enabled` to `true`, and inserted
   `1 | IGNITE_LIBRARY_NAMESPACE.IGNITE_SERIES_GROUPING_ENABLED_KEY | 1`.
   No restart.

Result, as the developer saw it: **Amazon series are grouped**, and a
"Group by series" switch appears in the full **Settings** page (but still
not in Quick Settings, so that tile list has another filter, which wasn't
investigated: `QuickSettingsWidgetProvider::getIconPanelWidgetList`). The
`kws-test series` books disappeared without a series tile taking their
place. They're probably hidden as members (`VISIBILITY_WITH_GROUPING = 1`),
and our container is missing something that the series-tile query needs.
Why is **out of scope** for this investigation, which is only about turning
grouping on.

Next steps, in order:

1. Find out which change does it: the `dcc` marketplace override, `dcc`
   `series.grouping.enabled`, the UserDefaults flag, or a combination.
   Also find out whether grouping only makes the Settings switch appear,
   or works without the switch being touched.
2. Then, if there's time, why Quick Settings still has no tile.
3. Then a developer script (checked in, with a `just` task, not shipped in
   the package) that turns grouping on and off using only the settings
   that step 1 shows are needed.

Undo:

```sh
sqlite3 /var/local/appreg.db "update properties set value='false' where handlerId='dcc' and name='series.grouping.enabled';"
sqlite3 /var/local/user_defaults.db "delete from user_defaults where ID=1 and Key='IGNITE_LIBRARY_NAMESPACE.IGNITE_SERIES_GROUPING_ENABLED_KEY';"
sqlite3 /var/local/appreg.db "delete from properties where handlerId='dcc' and name='series.grouping.supported.marketplaces';"
```

What might reset it (unconfirmed):

- `dcc` rows are written from the server's dynamic config, so a refresh
  could set `series.grouping.enabled` back to `false` or drop the
  marketplace override. Check after a reboot and a sync.
- UserDefaults are wiped on deregistration ("Wiped all user-defaults data
  on deregistration"), and Ignite's "Cleaning user preferences for library
  namespace" routine clears `IGNITE_SERIES_GROUPING_ENABLED_KEY` too.

### Test data left on the device for that investigation

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
