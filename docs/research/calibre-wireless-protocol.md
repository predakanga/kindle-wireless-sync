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
| DISPLAY_MESSAGE | 17 | 1 = password error, 2 = update needed, 3 = toast |
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

- Stock Calibre supports two-way sync of **read status (bool)** and **last
  read date (datetime)**. The device sets `isReadSyncCol` and
  `isReadDateSyncCol` to the names of the user's Calibre custom columns and
  returns `_is_read_` and `_last_read_date_` in each `GET_BOOK_COUNT` record.
  Calibre checks that the columns exist and have the right types.
- **Percent progress and rating can't go back to Calibre through the stock
  driver.** It has `CAN_SET_METADATA = []` and only syncs the two columns
  above. Those need a Calibre plugin (allowed by the requirements for stretch
  goals). The plugin could, for example, wrap or extend the smart device
  driver to accept extra fields in the book records.

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
  converted to AZW3. Whether to drop `mobi` from the list (which forces
  conversion to AZW3, the better format) is an open question in the plan.
- **lpaths:** the default template `{title} - {authors} ({id})` produces flat
  names, e.g. `Example Book - Jane Author (42).mobi`. No subfolders.
- **Metadata fields we receive:** `title`, `authors`, `author_sort`,
  `series`, `series_index`, `_series_sort_`, `uuid`, `lpath`, `size`,
  `last_modified`, `timestamp`, `pubdate`, `rating`, `pages`, `identifiers`,
  `comments`, `publisher`, `tags`, `languages`, `mime`, `user_metadata`
  (custom columns), `thumbnail` `[w, h, base64-jpeg]` at our requested
  `coverHeight` (500 → 320×500), and a `cover` path that is a temp file on the
  PC and useless to us.
