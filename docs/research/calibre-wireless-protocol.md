# Calibre wireless device protocol

Research for Phase 1, items 1, 2, 4 and 5. Sources:

- Calibre `src/calibre/devices/smart_device_app/driver.py` (master @ `a96f76a`,
  2026-10-15). Calibre's own name for this driver is "SmartDevice App
  Interface".
- KOReader `plugins/calibre.koplugin/wireless.lua` and `metadata.lua`
  (master @ `892034c`, 2026-10-14). This is a working client.
- Calibre `src/calibre/devices/kindle/driver.py`, the USB Kindle driver, used
  for comparison.

Line numbers below refer to `driver.py` at the commit above.

## Roles

**Calibre is the TCP server and the device is the client.** Calibre listens
(port 9090 by default, otherwise a random or user-fixed port) and the device
connects to it (`_startup_on_demand`, ~L1928). After the connection is made,
**Calibre sends every request** and the device only answers. The device never
starts an operation of its own.

So "allowlisting a Calibre connection" means the Kindle choosing **which
Calibre servers it will connect to**. It does not mean accepting inbound
connections.

## Discovery

Calibre supports two discovery methods:

1. **UDP broadcast.** Calibre binds the first free port of
   `54982, 48123, 39001, 44044, 59678` (L255). The client broadcasts any
   payload to each port. KOReader sends `"hello"` to `255.255.255.255` from
   local port 8134. Calibre replies with:
   ```
   calibre wireless device client (on <short-hostname>);<content-server-port>,<smartdevice-port>
   ```
   (L130). The content-server port may be empty.
2. **mDNS.** Calibre publishes the service type `_calibresmartdeviceapp._tcp`
   on the listen port (L2031).

A user can also give a host and port by hand. KOReader supports this, and it
is needed when broadcast doesn't work, for example across subnets.

## Framing

- Control messages are `<decimal length><JSON>` with no separator. The JSON is
  always `[opcode, {payload}]` and the length counts the JSON bytes only
  (L690, `_read_string_from_net` L623). Example: `34[0, {...}]`.
- Book contents are sent as **raw bytes** straight after a `SEND_BOOK`
  message, exactly `length` bytes long, in chunks of `maxBookContentPacketLen`
  (L776). Uploads to Calibre via `GET_BOOK_FILE_SEGMENT` work the same way in
  the other direction.
- Calibre uses its own JSON encoding for metadata (`JsonCodec`).
  `thumbnail` is `[width, height, base64-jpeg]`, and dates are ISO strings.

## Opcodes

| Op | # | Direction / meaning |
|---|---|---|
| OK | 0 | Device's reply |
| SET_CALIBRE_DEVICE_INFO | 1 | Calibre → device: persist the `driveinfo` record |
| SET_CALIBRE_DEVICE_NAME | 2 | Rename device |
| GET_DEVICE_INFORMATION | 3 | Device returns `device_info` (incl. `device_store_uuid`), `version`, `device_version` |
| TOTAL_SPACE / FREE_SPACE | 4 / 5 | Storage numbers |
| GET_BOOK_COUNT | 6 | Device streams the count, then one record per book |
| SEND_BOOKLISTS | 7 | Calibre announces a metadata push, **including the collections map** |
| SEND_BOOK | 8 | Header for a book transfer, followed by raw bytes |
| GET_INITIALIZATION_INFO | 9 | Handshake |
| BOOK_DONE | 11 | (unused by current Calibre) |
| NOOP | 12 | Keepalive. Also carries `ejecting`, `count` and `priKey` sub-messages |
| DELETE_BOOK | 13 | `{lpaths: [...]}`; device replies with one OK `{uuid}` per lpath |
| GET_BOOK_FILE_SEGMENT | 14 | Calibre pulls a file off the device |
| SEND_BOOK_METADATA | 16 | One per book after SEND_BOOKLISTS; no reply |
| DISPLAY_MESSAGE | 17 | 1 = password error, 2 = update needed, 3 = toast. **Calibre waits for a reply** (`_show_message` uses `_call_client`, and the device socket has no timeout), so the device must answer `OK`, or Calibre's device thread hangs |
| CALIBRE_BUSY | 18 | Another device is already connected |
| SET_LIBRARY_INFO | 19 | Library name, UUID, field metadata |
| ERROR | 20 | Device → Calibre, refuses a SEND_BOOK (Calibre ≥ 4.18) |
| GET_COLLECTIONS / UPDATE_COLLECTIONS | 21 / 22 | Defined but **never sent by stock Calibre** |

## Session sequence (from `open`, `get_device_information`, `books`, `sync_booklists`)

1. Device connects over TCP.
2. Calibre sends `GET_INITIALIZATION_INFO` with `serverProtocolVersion: 1`,
   `validExtensions`, `passwordChallenge`, `currentLibraryName`,
   `currentLibraryUUID`, `calibre_version`, and date formats.
   The device replies `OK` with its capabilities (see below). If there's a
   password, Calibre checks `passwordHash` and on mismatch sends
   `DISPLAY_MESSAGE{messageKind:1}` and closes the connection.
3. `GET_DEVICE_INFORMATION`, then `SET_CALIBRE_DEVICE_INFO`. The device must
   **persist** the driveinfo, because its `device_store_uuid` keys Calibre's
   metadata cache.
4. `SET_LIBRARY_INFO`, then `TOTAL_SPACE`/`FREE_SPACE`, then `GET_BOOK_COUNT`.
   The device sends `{count, willStream, willScan}` and then `count` book
   records. With `canUseCachedMetadata`, each record is a short summary
   (`priKey`, `uuid`, `lpath`, `last_modified`, `extension`, plus sync
   fields). Calibre then asks for full metadata of cache misses with
   `NOOP{count}` and `NOOP{priKey}`.
5. `SEND_BOOKLISTS` followed by N × `SEND_BOOK_METADATA` (no replies).
6. Whenever the user sends books: `SEND_BOOK` (the device replies OK with a
   possibly changed `lpath` if `canSendOkToSendbook`), then the raw bytes.
   After each batch, steps 4 and 5 run again.
7. Idle: Calibre sends a `NOOP` roughly every 5 s of activity probing
   (`detect_managed_devices`). It can disconnect after 30 minutes of
   inactivity if the user enables that option. On eject it sends
   `NOOP{ejecting:true}`.

Calibre only serves **one wireless device at a time**. A second device gets
`CALIBRE_BUSY`.

## Handshake capabilities the device must declare

Calibre refuses the device unless `canStreamBooks`, `canStreamMetadata`,
`canReceiveBookBinary` and `canDeleteMultipleBooks` are all true (L1153).
The other fields we care about:

| Field | Effect |
|---|---|
| `acceptedExtensions` | **Becomes Calibre's `format_map` for this device**, in priority order. Calibre then offers or auto-converts to the first acceptable format when sending. |
| `extensionPathLengths` | Path-length reserve per extension |
| `useUuidFileNames` | Calibre names files `<uuid>.<ext>` instead of using its save template |
| `coverHeight` / `coverWidth` | Size of the `thumbnail` JPEG inside the metadata |
| `canUseCachedMetadata`, `cacheUsesLpaths` | Lets Calibre skip resending metadata it already has |
| `canSendOkToSendbook` | The device may reject a book or rewrite its lpath |
| `canAcceptLibraryInfo` | Enables `SET_LIBRARY_INFO` |
| `isReadSyncCol`, `isReadDateSyncCol` | Names of Calibre custom columns for two-way read status and read date |
| `setTempMarkWhenReadInfoSynced` | Marks synced books in Calibre |
| `willAskForUpdateBooks` | Calibre re-sends a format whose mtime is newer than `_format_mtime_` |
| `passwordHash` | `sha1_hex(password + challenge)` |
| `maxBookContentPacketLen` | Chunk size (must be > 0) |
| `deviceKind`, `deviceName`, `appName`, `ccVersionNumber` | Shown in Calibre |

## Findings against the requirements

### Base requirements: no blockers found

- **Discovery and connection** work with stock Calibre. The user has to
  enable "Start wireless device connection" in Calibre.
- **Format conversion:** we declare only formats the built-in reader can open
  (e.g. `azw3`, `mobi`, `kfx`, `pdf`, `txt`) and Calibre converts EPUB to
  AZW3 when sending. Nothing is needed on the PC.
- **Covers:** Calibre sends a JPEG `thumbnail` in each book's metadata at our
  requested size. Over USB, Calibre's Kindle driver writes that to
  `system/thumbnails/thumbnail_<uuid>_<cdetype>_portrait.jpg` and keeps a
  backup in `amazon-cover-bug/` because the firmware deletes them
  (`kindle/driver.py` `upload_kindle_thumbnail`, `sync_cover_thumbnails`).
  **Over wireless none of that happens, so the client has to do it.** The
  client must read `uuid` and `cdetype` from the book's EXTH header (or the
  KFX container) itself.
- **APNX page numbers:** the USB driver generates `.apnx` files on the PC,
  but the wireless driver doesn't. This is optional, so park it.

### Allowlisting and security

- The password scheme only authenticates the **device to Calibre**. The
  device can't tell that the server it reached is the real Calibre, since
  Calibre never proves it knows the password.
- What the device can use to identify a Calibre instance: the hostname from
  the broadcast reply, the IP and port, and `currentLibraryUUID` /
  `currentLibraryName` from the handshake. Only the library UUID is stable,
  and it's spoofable by anyone on the LAN who has seen it. An allowlist keyed
  on host plus library UUID stops accidental connections. It doesn't stop a
  hostile peer on the same network. Covered in a decision.
- Any server can send arbitrary `lpath`s. The client **must** validate paths
  (relative, no `..`, no NUL, extension on the allowed list) and keep writes
  inside the books directory. KOReader's `isSafeLpath` plus its realpath
  containment check is a good model.

### Stretch 1: Series → collections, possible with stock Calibre

Calibre's wireless driver setting "Comma separated list of metadata fields to
turn into collections" (default empty) makes `SEND_BOOKLISTS` carry
`collections: {name: [lpath, ...]}` (L1461). With the user set to `series`, we
get series collections with no plugin. We would still apply them to the
Kindle's own collection store, which is a separate research item. The
opcodes `GET_COLLECTIONS`/`UPDATE_COLLECTIONS` are not used by Calibre, so
collections only flow Calibre → device.

### Stretch 2: progress and rating, partly possible with stock Calibre

- Calibre has no read flag or last-read date of its own. The wireless
  driver can sync **two custom columns the user creates**: a Yes/No column
  for **read status** and a Date column for the **last-read date**. The
  device names them in its handshake (`isReadSyncCol`,
  `isReadDateSyncCol`) and returns `_is_read_` and `_last_read_date_` in
  each `GET_BOOK_COUNT` record. Calibre checks that the columns exist and
  have the right types. Without those columns, nothing is synced.
- **Percent progress and rating can't go back to Calibre through the stock
  driver.** It has `CAN_SET_METADATA = []` and only syncs the two columns
  above. Those need a Calibre plugin (allowed by the requirements for stretch
  goals). The plugin could, for example, wrap or extend the smart device
  driver to accept extra fields in the book records.

#### Where Calibre keeps reading progress (2026-10-16)

Read from Calibre's source on GitHub (master, October 2026):
- Calibre does store progress, in the library's `last_read_positions` table
  (book, format, user, device, CFI, epoch, `pos_frac` 0–1). The template
  function `reading_progress()` reads it (`utils/formatter_functions.py`,
  `BuiltinReadingProgress`). The custom-column dialog's "Read progress"
  preset is a composite column of `{id:reading_progress()}`
  (`gui2/preferences/create_custom_column.py`). It isn't a standard
  column. The developer's test column `#read_progress_custom` is one of
  these, for research only.
- **Writers:** Calibre's own viewer (`gui2/viewer/integration.py`) and the
  Content server's reader, through `POST
  /book-set-last-read-position/{library_id}/{book_id}/{fmt}` with a JSON
  body of `device`, `cfi` and `pos_frac` (`srv/books.py`). The wireless
  driver never touches it.
- So a device could only set progress without a plugin over a **second
  connection** to the Content server. The developer ruled that out
  (2026-10-16): one transport between Calibre and the daemon, for
  security.

#### Can a plugin extend the wireless driver? Yes (2026-10-16)

Both ways below keep the single wireless connection.
- **Replacing the driver.** The GUI's device manager registers every
  *enabled* device plugin whose `is_dynamically_controllable()` returns a
  name. The Connect/share menu and autostart then drive the one named
  `smartdevice` (`gui2/device.py` `run()` and `dynamic_plugins`;
  `gui2/ui.py` `start_smartdevice`). A device-driver plugin that subclasses
  `SMART_DEVICE_APP` can take its place once the user disables the
  built-in "SmartDevice App Interface". It can then read any extra field,
  or opcode, we send. The cost: it overrides private methods of a large
  class with no stable API, so it can break with Calibre updates.
- **An interface plugin reading the device book list.** `raw_to_book`
  (`ebooks/metadata/book/json_codec.py`) keeps every key of a book record
  as an attribute. So extra fields from the daemon (say a progress fraction)
  reach Calibre's in-memory device books, where a GUI plugin could copy them
  into the library, without touching the driver. **The catch is the metadata
  cache**, which we use (`canUseCachedMetadata: true`). For a cached book
  (same uuid, lpath and `last_modified`, `_metadata_in_cache`), `books()`
  copies only `_is_read_`, `_sync_type_`, `_last_read_date_` and
  `_format_mtime_` from the device's record. Everything else is dropped. An
  extra field only arrives when the cache misses. That means the cache would
  be turned off (full metadata on every connect), or `last_modified` would
  be changed whenever progress changes, which also makes Calibre resend that
  book's metadata.
- Either plugin could write progress where Calibre itself keeps it
  (`db.set_last_read_position(..., device=<Kindle name>, pos_frac=...)`),
  so the standard `reading_progress()` would work with no custom column. A
  rating would go to Calibre's built-in `rating` field.

## Things the client must persist on the Kindle

- The `driveinfo` record (`device_store_uuid` etc.) from
  `SET_CALIBRE_DEVICE_INFO`.
- A book list holding at least `uuid`, `lpath`, `last_modified`, size, title,
  authors and series fields, so we can answer `GET_BOOK_COUNT` and
  `DELETE_BOOK`. KOReader stores Calibre's own `metadata.calibre` and
  `driveinfo.calibre` JSON files in the inbox directory, which is the same
  layout Calibre uses for USB devices. Whether to share those files with the
  USB driver is an open question.

## Observed against Calibre 9.16.0 (2026-10-15)

From live sessions between `kws connect` on the dev Mac and the user's
Calibre (anonymized in `tests/fixtures/calibre-9.16-session.jsonl`):

- **Discovery:** a broadcast to `255.255.255.255` reached Calibre on the same
  machine. Calibre had bound UDP 54982, the first broadcast port, and TCP
  9090.
- **Connect sequence:** `GET_INITIALIZATION_INFO` → `GET_DEVICE_INFORMATION` →
  `SET_CALIBRE_DEVICE_INFO` (Calibre assigns `device_store_uuid` on first
  contact) → `FREE_SPACE` → `SET_LIBRARY_INFO` → `GET_BOOK_COUNT` →
  `NOOP{count}` → `SEND_BOOKLISTS`. `TOTAL_SPACE` was never requested.
- **Reconnect with cached metadata:** we report `{priKey, uuid, lpath,
  last_modified, extension}` for each book. Calibre answered
  `NOOP{count: 0}`, meaning every book was a cache hit and no full metadata
  was requested.
- **Idle:** Calibre sends `NOOP {}` about every 3 s while connected.
- **Sending:** each batch is preceded by `FREE_SPACE` and followed by
  `SEND_BOOKLISTS` with `count: 0`. Calibre already knows the metadata it just
  sent.
- **Format choice:** Calibre sends the **first format in our
  `acceptedExtensions` that the book already has**, and converts only when
  it has none. Books that had MOBI and EPUB arrived as **MOBI** even though
  `azw3` is first in our list. A book without any of our formats was
  converted to AZW3. We keep `mobi` in the list (ADR 0007).
- **lpaths:** the default template `{title} - {authors} ({id})` produces flat
  names, e.g. `Example Book - Jane Author (42).mobi`. No subfolders.
- **Metadata fields we receive:** `title`, `authors`, `author_sort`,
  `series`, `series_index`, `_series_sort_`, `uuid`, `lpath`, `size`,
  `last_modified`, `timestamp`, `pubdate`, `rating`, `pages`, `identifiers`,
  `comments`, `publisher`, `tags`, `languages`, `mime`, `user_metadata`
  (custom columns), `thumbnail` `[w, h, base64-jpeg]` at our requested
  `coverHeight` (500 → 320×500), and a `cover` path that is a temp file on the
  PC and useless to us.
- **Device view:** Calibre's device view showed the right titles and covers
  for the books we hold (checked by the developer). So the cached-metadata
  path works and keeps Calibre's own cover cache intact, even though we
  store our records without thumbnails.

## Ending a session from the device (2026-10-18)
PLAN Later item 10 asks whether the device can end a session more safely
than "reply to a keep-alive after 10 s of quiet, then close" (ADR 0017,
ADR 0020). Read from Calibre 9.16.0: `smart_device_app/driver.py` and
`gui2/device.py` (line numbers are from that tag).

**The device can't ask to be ejected.** Calibre sends every request and the
device only replies. Nothing reads an unsolicited message: while idle,
`detect_managed_devices` (driver.py 1014) only `select`s the socket, and
takes "readable" to mean the device has gone ("this can only be a
disconnect notification"). No reply field makes Calibre eject or
disconnect. Outside the handshake, the only reply it interprets is `ERROR`
during a book upload (`_put_file`, 765), which raises `UserFeedback`, and
the GUI shows that as an error dialog.

**When the keep-alive is sent, and when jobs run.** Calibre's device thread
(`DeviceManager.run`, device.py 438) loops:
1. `detect_device()` (325). For a connected wireless device this calls
   `detect_managed_devices`, which sends the keep-alive: a `NOOP` with an
   empty payload. Within `SEND_NOOP_EVERY_NTH_PROBE` (5) seconds of the last
   other request it sends one on every probe; after that only when the
   seconds since then are 1 more than a multiple of 5. Other probes only
   `select`.
2. Run every queued job, until none are left. Steps a running job adds for
   itself (`create_job_step`, 521) go to `job_steps` and run straight after
   it. Jobs that GUI callbacks queue later go to `jobs`.
3. Sleep 2 s (`sleep_time`), unless it ran a job, then go back to 1.

So **a keep-alive never arrives during a job**: detection and jobs take
turns on one thread. The other `NOOP`s have a payload: `count` and `priKey`
while listing books (`books`, 1417), and `ejecting` (`eject`, 1531).

**What a failed keep-alive does.** `detect_managed_devices` wraps the
keep-alive in `except Exception: self._close_device_socket()` and returns
`None`. If the device closes the connection instead of replying,
`_read_string_from_net` gets 0 bytes and `_receive_from_client` raises
`ControlError`, which is caught there. `detect_device` then calls
`connected_device_removed` (293), the same path as a device that drops off
Wi-Fi. No dialog appears for the disconnect itself.

**Jobs already queued are the remaining risk.** `connected_device_removed`
aborts every job still in `jobs` with "Device no longer connected."
(`DeviceJob.abort`). Aborting marks the job failed and calls its callback,
and the callbacks pass failed jobs to `device_job_exception`, which shows
the device error dialog. A job is queued but not yet run only if it
arrived after the previous turn's jobs were done: in the 2 s sleep before
this keep-alive.

**Compared with what we do now** (reply, then close):

| | Reply, then close (now) | Close instead of replying |
|---|---|---|
| Job queued in the 2 s before the keep-alive | Runs against a closed socket: fails part-way ("Device closed the network connection"), error dialog | Aborted before it starts, error dialog |
| Job queued after it, before Calibre's next probe | Aborted at that probe, error dialog | Can't happen: Calibre has already removed the device |
| Calibre notices the disconnect | At its next `select` (up to 2 s later) | At once |

**Closing instead of replying is strictly better, but not risk-free.** It
removes the window after the close and turns part-way failures into clean
aborts. The window before the keep-alive (a user's action, or a GUI callback
queuing a follow-up job, in the 2 s before it) can't be closed from the
device: nothing tells the device a job is queued. The 10 s quiet rule should
stay: right after a job, keep-alives come on every probe, while GUI
callbacks (for example `books_uploaded` queuing `sync_booklists`) may be
about to queue the next job.
