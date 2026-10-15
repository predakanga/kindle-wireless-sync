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

Unless a source says otherwise, none of this has been confirmed on **FW 5.20.1**.
The on-device probes at the end are needed to close Phase 1.

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
