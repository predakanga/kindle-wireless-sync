# Project Plan

A KPM package for jailbroken Kindles that receives books from Calibre's
**Wireless device connection** (the "smart device" driver) and adds them to
the Kindle's **built-in** reader.

## Requirements

Base:

1. Written in Rust.
2. Ships as a package that KPM (Kindle Package Manager) can install.
3. Has an on-device UI where the user allowlists Calibre connections.
4. Books appear in the Kindle library as soon as each sync finishes, with no
   reboot or manual rescan.
5. Targets the built-in reader. KOReader and other third-party readers are out
   of scope.
6. The user's PC needs nothing except stock Calibre.

Stretch (a Calibre plugin is allowed for these):

1. Create Kindle collections from Calibre's Series metadata.
2. Send reading progress and rating back to Calibre.
3. Use the Kindle's own device name as the device name shown in Calibre.
   Low priority.

## References

- Calibre source: https://github.com/kovidgoyal/calibre/tree/master
  (smart device driver: `src/calibre/devices/smart_device_app/`)
- Kindle Button Mapper: https://github.com/zampierilucas/kindle-button-mapper-rs
  (shows a Rust Kindle GUI app, KPM packaging and a justfile debug workflow)
- KOReader Calibre plugin: https://github.com/koreader/koreader/tree/master/plugins/calibre.koplugin
  (an existing client implementation of the wireless protocol)
- Kindle HID Passthrough: https://github.com/zampierilucas/kindle-hid-passthrough
  (shows a user-controlled service with start at boot or network-up)

## Phases

Each phase ends with its findings or decisions written down (see
[decisions/](decisions/)) and a commit. Later phases may change based on what
Phase 1 finds.

### Phase 0: Project setup ✅

- Repo, CLAUDE.md, this plan, decision log.

### Phase 1: Research and feasibility (no product code) ✅

The aim is to answer the open questions below and confirm that every base
requirement can be met without a Calibre plugin. If one can't, stop and
decide what to do before building anything.

1. **Calibre wireless protocol.** Read the smart device driver and the KOReader
   plugin to learn discovery (UDP broadcast), the TCP framing, the opcode set,
   the handshake and password challenge, how books are sent and deleted, and
   what metadata Calibre sends (series, series index, rating, `_last_read`
   fields and so on). Find out which side initiates the connection, since that
   determines what "allowlist a Calibre connection" means. Check what the
   driver lets the device declare: accepted formats, which drives Calibre's
   auto-conversion, plus path templates and cover sizes.
2. **Formats.** Confirm which formats the built-in reader opens when sideloaded
   (expected: AZW3/MOBI/KFX/PDF/TXT; EPUB is not expected to work), and whether
   the device can make Calibre convert EPUB to AZW3 automatically.
3. **Instant availability.** Find out how a sideloaded file gets into the
   Kindle library: the content catalog (`cc.db` / `com.lab126.ccat`), the
   scanner/indexer, and lipc/dbus events. Look for a supported way to register
   or rescan a single file without a full reindex or reboot. Also check how
   thumbnails/covers get generated for sideloaded books.
4. **Collections (stretch).** Find out how collections are stored and edited
   on current firmware, and whether a running process can create them and
   assign books to them.
5. **Progress and ratings (stretch).** Find where the built-in reader stores
   position and percent read (sidecar `.sdr` files, `cc.db` columns) and
   whether the wireless protocol can carry it back to Calibre
   (`_last_read`-style fields, or `SEND_BOOK_METADATA`-style opcodes), or
   whether a Calibre plugin is needed.
6. **Kindle platform.** From Kindle Button Mapper, learn the KPM package format
   and install hooks, the cross-compile target(s) (check armel vs armhf across
   firmware versions), GUI toolkit options and how an app is launched from the
   Kindle UI. From Kindle HID Passthrough, learn how a user-controlled service
   is registered and started at boot or network-up (see
   [0002](decisions/0002-run-lifecycle.md)). Also find out how Wi-Fi and sleep
   behaviour affect a long-lived connection.

**Exit criteria:** a written answer to each item, a feasibility verdict for
every base requirement, and a first set of architecture decisions.

**Status:** the desk research and on-device probes are done (see
[research/](research/), probed on 5.20.1 on 2026-10-15). **Phase 1 is
complete:** ADRs 0001–0005 are accepted.

| Requirement | Verdict so far |
|---|---|
| Rust | Feasible: static `armv7-unknown-linux-musleabihf`, as in the references |
| KPM package | Feasible: `.kpkg` tarball with `manifest.json` at its root |
| Allowlist UI | Feasible: WAF app plus localhost API ([0004](decisions/0004-ui-architecture.md)), trust model in [0003](decisions/0003-calibre-trust-model.md) |
| Immediate library availability | **Confirmed** (TXT and AZW3): the scanner indexes new, changed and deleted files in ~2 s. We must write the cover ourselves, before the rename ([0005](decisions/0005-library-registration-via-scanner.md)) |
| Built-in reader | Feasible: declare only AZW3/MOBI/KFX/PDF/TXT, and Calibre converts. Confirmed that sideloaded EPUBs are ignored |
| Stock Calibre only | Feasible: no plugin needed for any base requirement |
| Stretch: Series → collections | Feasible with stock Calibre (collections map in `SEND_BOOKLISTS`). Native series grouping is also possible. The write path must keep `cc.db` and `ksdk.asset.db` consistent: ChangeRequest API on :9101 (still to test) rather than direct SQL |
| Stretch: progress/rating | Read status and date work with stock Calibre. Percent read (`p_percentFinished`) needs a plugin. **No rating is stored on the device**, so rating sync is likely infeasible |

### Phase 2: Protocol client on the host

Build the Calibre wireless client as a plain Rust library and CLI that runs on
the dev machine. Test it against a real Calibre instance, with the Kindle
storage simulated in a local directory.

- Discovery, connect, handshake and password.
- Receive, store and delete books. Report the book list and free space.
- Integration tests against recorded and replayed protocol transcripts, so CI
  doesn't need Calibre.

**Exit criteria:** Calibre treats the host CLI as a connected device and can
send books to it and delete books from it.

**Status (2026-10-15):** exit criteria met against Calibre 9.16.0: discover,
connect, receive (with auto-conversion), list from cache on reconnect,
delete and eject. 32 tests, including a replay of two anonymized real
sessions. The Kindle binary is 528 KB.

### Phase 3: Running on the Kindle ✅

- Cross-compile and set up the build and deploy workflow (a justfile modelled
  on Kindle Button Mapper).
- Run the client on the device, write to the correct documents location, and
  trigger library registration so books show up immediately.
- Service layout per [0004](decisions/0004-ui-architecture.md): a daemon job
  plus an autostart task gated by a sentinel, files in `/var/local/kindle-wireless-sync/`. The
  daemon waits for Wi-Fi CONNECTED before it connects.

**Exit criteria:** a book sent from Calibre opens in the built-in reader
without a reboot.

**Status (2026-10-15):** done: deploy recipes, covers (EXTH key, written
before the book appears, backed up and restored), and an end-to-end run on
the device (receive, live appearance with covers, in-place update, delete).
Daemon mode (ADR 0008) works on the device against Calibre 9.16.0:
`find` and `allow` from the Kindle, automatic connection, Wi-Fi drop
detected at once (the socket is cut), reconnect about 5 s after Wi-Fi
returns, and staying disconnected after an eject until Wi-Fi cycles. The
upstart jobs (ADR 0004, ADR 0010) are installed on the device. Start and
stop work through `initctl`. With autostart on, the service started about
65 s after a reboot and connected to Calibre, and the crash-loop counter
cleared after 2 minutes. **Phase 3 is complete.** **Exit criterion met:** the
received test book (AZW3) opened in the built-in reader without a reboot
(screenshot, 2026-10-15).

### Phase 4: UI and packaging ✅

**Status (2026-10-15, in progress):** done:
- the localhost API (ADR 0011) in the daemon, with tests
- the WAF settings app (`package/ui/`), the appreg registration script, and
  the Library scriptlet with its icon
- deploy recipes (`just deploy-ui`, `just open-ui`)

Checked on the device: the app shows live status, Pause/Resume work, the
tile opens the app (starting the service if needed), and the `Origin`
values are as expected. Also checked: the on-screen keyboard types into the
Add server and password fields, Find libraries ("No new libraries found",
then found and allowed after a Forget), Add server with a bad address (an
error message and nothing stored), the password form (save, clear, cancel),
Forget (two taps) and the autostart toggle. These checks found and fixed
three problems: the keyboard covered the fields and Enter did nothing; a
Forget during a session was undone when the session ended; and a requested
reconnect waited out the backoff.

The KPM package (ADR 0012) is built by `just package`. KPM 0.2.2
on the device installed it over the development setup, upgraded it from a
repository, uninstalled it cleanly (twice) and installed it fresh.
**Exit criterion met (2026-10-15):** after a fresh KPM install, Find
libraries, Allow and a book sent from Calibre (MOBI) showed up in
the library, with only stock Calibre. Distribution: GitHub Releases plus a KPM
index on GitHub Pages, published by a release workflow (ADR 0014). The
first release, `v0.1.0`, is published, and installing it on the device from
the Pages repository worked. `v0.1.1` followed, and `kpm upgrade` from
0.1.0 to 0.1.1 worked on the device (ADR 0014).

**Adopting USB-sent books (ADR 0013, 2026-10-15):** done and tested on the
device. Books Calibre sent over USB are added to our book list at the start
of each session, from `metadata.calibre` or the file's EXTH, so Calibre sees
them as on the device and stops sending duplicates.


- On-device UI for discovered Calibre instances and the allowlist (with
  password if one is needed), service start/stop and autostart settings, and
  connection and sync status.
- KPM package with install, upgrade and uninstall that leaves the device
  clean.

**Exit criteria:** a fresh install from KPM, through allowlisting, to a first
sync works end to end with only stock Calibre.

### Phase 5: Stretch goals

- ✅ Series from Calibre as the Kindle's **native series grouping** (one tile
  per series, in series order), through the catalog's ChangeRequest
  endpoint (ADR 0016, released in 0.2.0). Decided 2026-10-16. Collections in
  general are on the "Later" list. History:
  **Status (2026-10-16):** research done
  (`research/kindle-library-integration.md`, "Native series on FW 5.20.1").
  Series data can be written correctly through ChangeRequest. But the
  reference device (AU marketplace) doesn't show series grouping at all, not
  even for Amazon series, because the feature is gated by remote config and
  a weblab. Paused while the developer looks into enabling it on their own
  Kindle, which is personal, not part of the package. Next: decide whether
  to ship series data anyway (useful wherever grouping is on), or bring
  collections forward. A test series is still on the device (see the
  research note).
  **Update (2026-10-16):** the developer has turned series grouping on for
  their device, in a separate session. The two test books disappeared from
  the Library, but the "kws-test series" tile didn't appear.
  **Update (2026-10-16):** fixed in the experiment. The Library hides series
  containers whose member counts are under 2, and a ChangeRequest `members`
  update sets them. With that, the tile showed and opened with both books
  in order. Member deletion degrades safely. Details are in the research
  note ("Making a sideloaded series show"). Test data removed.
  **Built (2026-10-16, ADR 0016; released in 0.2.0 the same day, and
  upgraded on the device through KPM):** series now sync from
  Calibre. On the device, a 4-book series merged into one ordered tile 15 s
  after the send, and deleting a book while viewing the series updated the
  view in place. A 2-book series formed too. Deleting one of its books
  dissolved it, which bounced the open view back to the Library. Series
  survive a reboot. After a sync, no series showed up on other devices or
  the iOS app, and nothing is queued for upload (see the ADR).
  For testing, `just series-grouping [status|enable|disable]` turns grouping
  on and off. Changes apply as soon as the Library redraws (go to Home and
  back), with no restart (2026-10-16).
- Progress and rating sync back to Calibre, with a Calibre plugin only if
  Phase 1 shows it is required.
  **Status (2026-10-16):** research done
  (`research/calibre-wireless-protocol.md`, "Where Calibre keeps reading
  progress" and "Can a plugin extend the wireless driver?"). Without a
  plugin, only read status and a last-read date can reach Calibre, through
  the wireless driver's optional sync of two custom columns the user
  creates. ADR 0017 (Accepted) sends them from the Kindle to Calibre, into
  `#kindle_read` and `#kindle_last_read`, for libraries marked writable
  (off by default). The other direction is on the Later list.
  Percent read needs a Calibre plugin, which is deferred (Later list).
  **Ratings are out of scope:** on 5.20.1 the Goodreads rating prompt only
  appears for Amazon books. The device probes are done (2026-10-16): the
  Kindle's read state and last-read time are in `cc.db`.
  **Built (2026-10-17, unreleased):** tested on the device against Calibre
  9.16.0. Kindle changes reached Calibre, an edit made in Calibre survived
  a reconnect, and deleting the columns stopped sync quietly. The test found
  three bugs, all fixed: an error dialog when reconnecting mid-job, books
  skipped because of Calibre's `"None"` dates, and unanswered
  `DISPLAY_MESSAGE`s hanging Calibre (ADR 0017, implementation notes).
- ✅ Kindle device name as the Calibre device name (ADR 0015, released in
  0.1.1). The questions below are answered there:
  - Where does the firmware store the user-set device name, and is there a
    lipc property or event for changes?
  - What happens when the name changes while connected or between
    connections? Calibre shows `deviceName` from the handshake and keeps
    `device_name` in the driveinfo it stores on the device.
  - Which characters and lengths does Calibre accept? Sanitize the name, and
    fall back to "Kindle" if it is empty or unusable.

### Phase 6: Other firmware (stretch stretch goal)

- Check whether the on-device integration points we rely on (catalog
  registration, collections, progress storage, service hooks, UI) exist on
  firmware and models other than the reference device. Record what differs.
  See [0001](decisions/0001-target-device-and-firmware.md).

## Later: to revisit once the goals above are met

Not part of any phase. Pick these up after the base and stretch goals
(Phases 5–6) are done.

1. **KPM bug: a local `.kpkg` can't replace an installed package.** KPM 0.2.2
   compares installed ids with the `file://` URL (ADR 0012, findings).
   Report it upstream (KindleModding/KPM, with a minimal reproduction), and
   decide how we ship upgrades until it's fixed (see the distribution open
   question).
2. **Review the UI.** Usability on e-ink, and the actual wording of every
   label, hint and message. Consider moving the text into a translation
   file. Known item: status messages disappear after 6 s, which is easy to
   miss. Also: let the user pick the read-sync columns from a list of the
   library's columns (Calibre sends them in `SET_LIBRARY_INFO`), instead of
   the fixed names in ADR 0017, and a toggle for each library's
   `writable` setting (command line only until then).
3. **Kindle collections from Calibre fields** (series, tags, custom
   columns). The catalog's ChangeRequest endpoint supports them on 5.20.1:
   `insert` a `Collection`, then `addCollectionMember`. That's the command
   the Library app sends. Cloud collections may sync to the user's other
   devices, empty there, since sideloaded books don't sync; check that
   first. Deferred on 2026-10-16: Series uses the native series grouping
   instead (see Phase 5).
4. **A script that turns series grouping on or off**, packaged but **not
   shown in the UI**. Before it changes anything, it requires the user to
   pass their device's UUID as an argument, as a deliberate confirmation
   (like typing a repo's name to delete it), so users know what they're
   doing. It changes Amazon-side settings, so document the risk. It builds
   on the developer's own enabling work (2026-10-16). Still to decide:
   which identifier counts as the "device UUID". Start from the dev tool
   `tools/series-grouping.sh` (2026-10-16): only two `appreg.db` values are
   needed (research note, "Which change does it").
5. **A Calibre plugin for percent read** (deferred 2026-10-16).
   It must extend the wireless connection, not add a second one: either
   replace the wireless driver with a subclass, or read extra fields from
   the device book list. It would write progress into Calibre's own
   `last_read_positions`, so `reading_progress()` works. Trade-offs are in
   `research/calibre-wireless-protocol.md`, "Can a plugin extend the
   wireless driver?". Ratings are out of scope (ADR 0017).
6. **Send read status from Calibre to the Kindle** (deferred 2026-10-16).
   ADR 0017 only goes from the Kindle to Calibre. A ChangeRequest `update`
   of `readState` (1 = read, 3 = unread) works on 5.20.1, and the Library
   shows it at once (research note, "Read state probe"). First check
   whether a read state set this way is reported to Amazon.
7. **A tap-debugging helper in this repo.** UI checks over SSH currently
   rely on Kindle Button Mapper's `scripts/tap.sh`, which happens to be
   installed on the reference device. Add our own (or a vendored, credited
   copy, after checking its licence), plus a recipe that taps, screenshots
   and fetches the PNG.
8. **Reconnect after Calibre restarts.** Quitting Calibre ejects the
   device, and an eject keeps us disconnected until Wi-Fi cycles or Connect
   now (ADR 0008). Restarting Calibre, which adding columns requires, left
   the Kindle disconnected (developer, 2026-10-17). Find out whether a
   shutdown eject can be told apart from a user's eject.
9. **Test KFX.** ADR 0007 accepts KFX for users with Calibre's KFX Output
   plugin, but it has never been tested (developer, 2026-10-17). Check
   receiving a KFX book, its cover (`KeySource::AfterIndex`, which reads the
   content key after indexing), deleting and replacing it, whether the file
   carries Calibre's book UUID (so adoption, ADR 0013, could recover it), and
   what linking a different library does to it (ADR 0018).

## Open questions

Track these here until they become decisions.

- Should we also *write* the USB driver's `metadata.calibre`, so that USB
  connections see books we received over Wi-Fi? We keep our own state in
  `books.json`/`driveinfo.json`, and since ADR 0013 we only *read*
  `metadata.calibre` to adopt USB-sent books. Calibre's USB driver also
  reads metadata from files it doesn't know, so this may not be needed.
- What does a running service cost in battery, and does it stop the device
  from sleeping?
