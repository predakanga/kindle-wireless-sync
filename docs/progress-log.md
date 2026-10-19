<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Progress log

The full status notes from `PLAN.md`, moved here word for word on
2026-10-19 so the plan stays short. `PLAN.md` keeps a summary of each, and
links here. New detail belongs in ADRs (implementation notes) or research
notes; this log keeps the history as it was written.

<a id="phase-1"></a>
## Phase 1: Research and feasibility

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

<a id="phase-2"></a>
## Phase 2: Protocol client on the host

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

<a id="phase-3"></a>
## Phase 3: Running on the Kindle

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

<a id="phase-4"></a>
## Phase 4: UI and packaging

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

**Built (2026-10-17, unreleased): ADR 0018.** One linked library replaces
the allowlist, in the config, the command line (`link`, `unlink`), the API
(`/link`, `/unlink`) and the app. The daemon and API were tested on the
emulated Kindle with the Calibre harness (ADR 0018, implementation notes).
That test found and fixed a gap: reset records kept the old library's
date, so Calibre never resent their metadata. The app works on the emulator
too, once a packaging bug was fixed (its files were readable only by root;
ADR 0012). Checked on the device (2026-10-17): a KPM upgrade from 0.2.0,
linking the same library again without a reset, and the app; that found
and fixed an upgrade leaving the old app open (ADR 0012).

**UI review (2026-10-17 and 2026-10-18): ADR 0020, accepted.** The app
becomes pages that each fit one screen, and linking first checks the
library (password, columns) so read sync can be set up with the user's own
columns before connecting. A clickable prototype settled the design
(`tools/ui-prototype`, `just ui-prototype`). Replacing a library now keeps
series (ADR 0018, amended).

**Built (2026-10-18, unreleased): ADR 0020.** The check, the API, the new
app and the command-line options. Tested with unit tests, the harness
Calibre, end to end on the Mac, and on the emulated Kindle (ADR 0020,
implementation notes). Checked on the device, then **released as 0.3.0**
(2026-10-18), with read sync (ADR 0017) and ADR 0018. The device test also
fixed KFX keys and covers (ADR 0016). **0.3.1** followed the same day:
deleted books leave the Library at once (ADR 0005). **0.3.2** (also
2026-10-18) reconnects when Calibre comes back after quitting (ADR 0021),
and leaves sessions at a keep-alive, with a graceful Disconnect (ADR 0022).
Its first release run failed on a timing-dependent test, so the `v0.3.2`
tag was moved to the commit that fixed the test, before anything was
published.
**0.4.0** (2026-10-19) adds the storage folder: a folder picker in the
app's settings, new books going into the chosen folder, and updates
staying in place (ADR 0023).

<a id="phase-5"></a>
## Phase 5: Stretch goals

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

<a id="phase-6"></a>
## Phase 6: Other firmware

**Compatibility harness** (2026-10-19): `tools/compat/compat.py` (`just
compat MODEL VERSION`, README there) boots a model and firmware on
eink-emulator, creating and jailbreaking the instance on first use, and
prints one JSON line of check results. It first also installed the
package with KPM and exercised the service and app; after the 5.19.2
findings below, the developer narrowed it to probing the firmware's APIs
and tools (2026-10-19), since the emulator's own flakiness made the
end-to-end checks noisy. The results below mention checks it no longer has.

- **Paperwhite 6, 5.20.1** (`pw6-5201-jb`): all 25 checks pass. Every page
  fits; the tightest, the storage folder picker with ten folders and
  paging, has 95 px to spare in the app's 1272×1480 viewport. The
  on-screen keyboard starts 896 px down the app. mesquite doesn't shrink
  the viewport for it, so the harness finds it in the screenshot.
- **Basic 2016 (KT3), 5.16.2.1.1** (`kws-kt2016-516211`): not
  compatible, as expected for a soft-float device:
  - No `/lib/ld-linux-armhf.so.3`; jb.sh installs the `kindlepw2` KPM.
    KPM 0.2.2 still installed our kindlehf-only package from a local file,
    and the static binary ran: the service started and the app opened.
  - sqlite3 is 3.26.0, which has no `-json`, so every catalog query in
    `catalog.rs` fails (series retries every 15 s).
  - `Entries` has no `p_lastOpenTime` (read sync, ADR 0017).
  - The screen is 600×800; the app gets 600×680. Every page fits, the
    tightest with 22 px to spare. Focusing a field from script doesn't open
    the keyboard there, so the keyboard check can't measure it.
  - After a KPM install, the scanner's posts to the catalog were refused
    and books in new subfolders weren't added: the same scriptlet race as
    on 5.19.2, below.

- **Paperwhite 6, 5.19.2** (developer's run, 2026-10-19): two failures,
  both investigated on `kws-pw6-5192`.
  - **Books in new subfolders weren't added: a scriptlet race.**
    `install.sh` renamed `WirelessSync.sh` into `documents/`, and KPM
    rescans the library as soon as the hook returns (`doFullScan`, KPM
    `install.c`). When the rescan finds the scriptlet before the scanner
    has added it, the scanner sends one ChangeRequest with two inserts for
    it. The second fails (`SQL logic error` in `change.lua`, debug logging
    on `com.lab126.ccat`), the handler rolls back the batch, and the
    scanner resends the same batch on every later scan until a reboot,
    even after the file is gone. Books it finds by a full scan, as for a
    new folder, never get in; a book noticed directly in `documents/`
    still does. Reproduced without our install (copy a scriptlet, rescan at
    once). Not seen on 5.20.1.
    **Fix:** `install.sh` writes the scriptlet only if it changed, and
    waits (up to 30 s) until `cc.db` lists it at its new modification
    time. With it, three fresh installs had no refused posts, against 17 to
    153 before. Rapid uninstall-and-reinstall cycles (every 15 to 20 s)
    still hit it, since KPM's rescan after the uninstall can overlap the
    next install; waiting for `fullScanStatus` to be 0 first didn't help,
    so that isn't in. Upgrades don't rewrite the scriptlet.
  - **Uninstall stopped after removing the upstart jobs** (KPM exit 255,
    56 s), leaving the app registered, the tile and our directory. Not
    reproduced in nine more uninstalls (1 to 2 s each). The only step in
    that window that can fail is `initctl reload-configuration` (`mntroot`
    always exits 0, and sqlite3 fails at once on a lock), and two 25 s
    D-Bus timeouts plus the 2 s retry fit the time. **Fix:** a failed
    reload no longer stops the uninstall (it only makes upstart forget
    jobs whose files are gone). KPM shows a hook's output only when it
    succeeds: to see where one fails, prepend `exec 2>FILE; set -x` to the
    extracted copy in `/mnt/us/kmc/kpm/packages/<id>/`.
  - Found on the way: `series --remove-all` gave up on a locked catalog
    (now retried, as other catalog writes are), and `install.sh` failed
    with "Job is already running" when its running-check missed the
    service (now restarts it).
  - With the fix, one 5.20.1 run found the scriptlet missing right after
    an uninstall and reinstall: the `cmp` check had kept the old copy,
    which was then gone. Not reproduced in seven more; `install.sh` now
    writes it again if it's missing before the final wait.
  - **The emulator itself was unreliable under load** (Mac load about 9):
    a stock `curl` died with a glibc malloc assertion, the loader once
    couldn't open `/usr/lib/libenvload.so` during a remount, commands
    occasionally died without an exit status (ssh exit 255, no output),
    and the guest rebooted once on its own. These explain the scattered
    one-off failures; rerun before reading anything into one.

## Later items

<a id="later-1"></a>
### Later 1

1. **KPM bug: a local `.kpkg` can't replace an installed package.** KPM 0.2.2
   compares installed ids with the `file://` URL (ADR 0012, findings).
   Report it upstream (KindleModding/KPM, with a minimal reproduction), and
   decide how we ship upgrades until it's fixed (see the distribution open
   question).

<a id="later-2"></a>
### Later 2

2. **Review the UI.** Usability on e-ink, and the actual wording of every
   label, hint and message. Consider moving the text into a translation
   file. Known item: status messages disappear after 6 s, which is easy to
   miss. (The column picker and the read-sync toggle came with ADR 0020.)
   Home's status with its two-line detail under it is three lines, which
   the developer finds a bit much (2026-10-18).

<a id="later-3"></a>
### Later 3

3. **Kindle collections from Calibre fields** (series, tags, custom
   columns). The catalog's ChangeRequest endpoint supports them on 5.20.1:
   `insert` a `Collection`, then `addCollectionMember`. That's the command
   the Library app sends. Cloud collections may sync to the user's other
   devices, empty there, since sideloaded books don't sync; check that
   first. Deferred on 2026-10-16: Series uses the native series grouping
   instead (see Phase 5).

<a id="later-4"></a>
### Later 4

4. **A script that turns series grouping on or off**, packaged but **not
   shown in the UI**. Before it changes anything, it requires the user to
   pass their device's UUID as an argument, as a deliberate confirmation
   (like typing a repo's name to delete it), so users know what they're
   doing. It changes Amazon-side settings, so document the risk. It builds
   on the developer's own enabling work (2026-10-16). Still to decide:
   which identifier counts as the "device UUID". Start from the dev tool
   `tools/series-grouping.sh` (2026-10-16): only two `appreg.db` values are
   needed (research note, "Which change does it").

<a id="later-5"></a>
### Later 5

5. **A Calibre plugin for percent read** (deferred 2026-10-16).
   It must extend the wireless connection, not add a second one: either
   replace the wireless driver with a subclass, or read extra fields from
   the device book list. It would write progress into Calibre's own
   `last_read_positions`, so `reading_progress()` works. Trade-offs are in
   `research/calibre-wireless-protocol.md`, "Can a plugin extend the
   wireless driver?". Ratings are out of scope (ADR 0017).

<a id="later-6"></a>
### Later 6

6. **Send read status from Calibre to the Kindle** (deferred 2026-10-16).
   ADR 0017 only goes from the Kindle to Calibre. A ChangeRequest `update`
   of `readState` (1 = read, 3 = unread) works on 5.20.1, and the Library
   shows it at once (research note, "Read state probe"). First check
   whether a read state set this way is reported to Amazon.

<a id="later-7"></a>
### Later 7

7. **A tap-debugging helper in this repo.** UI checks over SSH currently
   rely on Kindle Button Mapper's `scripts/tap.sh`, which happens to be
   installed on the reference device. Add our own (or a vendored, credited
   copy, after checking its licence), plus a recipe that taps, screenshots
   and fetches the PNG.

<a id="later-8"></a>
### Later 8

8. **Reconnect after Calibre restarts.** Quitting Calibre ejects the
   device, and an eject keeps us disconnected until Wi-Fi cycles or Connect
   now (ADR 0008). Restarting Calibre, which adding columns requires, left
   the Kindle disconnected (developer, 2026-10-17). Find out whether a
   shutdown eject can be told apart from a user's eject. **Built**
   (2026-10-18, ADR 0021): after an eject, the service asks Calibre's
   address by datagram, and if Calibre no longer answers it reconnects once
   it does. The harness Calibre passed the ADR's four cases with the
   service on the Mac (2026-10-18): an eject stays disconnected; a quit and
   relaunch, and stopping and starting the wireless connection, reconnect
   within one 10 s round; a restart on port 9191 connects there, and
   `config.json` records it. Searches now also send the datagram to the
   linked address directly, so this works across networks too (ADR 0021,
   amendment). The emulated Kindle, linked at 10.0.2.2, passed the eject
   and the quit-and-relaunch cases (2026-10-18): QEMU passes the datagram
   and Calibre's answer. The reference Kindle passed the quit and relaunch
   with the developer's own Calibre (2026-10-18): "Calibre has closed",
   then connected 4 s after Calibre answered, with no tap. An eject from
   that Calibre was recognised as one: Calibre answered the check, and the
   Kindle stayed disconnected. Done.

<a id="later-9"></a>
### Later 9

9. **Test KFX.** ADR 0007 accepts KFX for users with Calibre's KFX Output
   plugin (developer, 2026-10-17). Receiving works, and keys and covers now
   follow indexing (ADR 0016, "KFX books and their keys", 2026-10-18).
   Still to check: deleting and replacing a KFX book, whether the file
   carries Calibre's book UUID (so adoption, ADR 0013, could recover it),
   and what linking a different library does to it (ADR 0018). A KFX
   file's own title shows in the Library (one test book showed
   "Dungeon Crawler Carl - 01"), not Calibre's.

<a id="later-10"></a>
### Later 10

10. **End connections more reliably than waiting for a quiet moment.**
    Several things end a session on purpose: new read-sync columns (ADR
    0017), new settings, a check of the same Calibre, and Calibre switching
    to another library (ADR 0020). Each waits for a keep-alive after 10 s
    of quiet (3 s for a check), because dropping the connection mid-job
    makes Calibre show an error dialog. That's a heuristic: a job could
    still start between the keep-alive and the close. The developer finds
    it iffy (2026-10-18). Look for other options in Calibre's driver
    (`smart_device_app/driver.py`): for example, whether the device can ask
    Calibre to eject it, or a reply that makes Calibre close the connection
    itself. **Researched 2026-10-18** (`research/calibre-wireless-protocol.md`,
    "Ending a session from the device"): there's no device-initiated eject,
    but closing the connection *instead of* replying to the keep-alive is
    safer than replying and then closing. Calibre treats it like a device
    that left Wi-Fi, and no job can be running then. Jobs queued in the 2 s
    before it are still aborted with an error dialog. **Built** (ADR 0022):
    the session and checks leave that way, and Disconnect, Forget library, a
    new password and relinking leave at the next keep-alive (15 s at most).
    Tested with the harness (2026-10-18; ADR 0022, implementation notes):
    no dialog in any case. Done.

<a id="later-11"></a>
### Later 11

11. **Present the Series field back to Calibre** (developer, 2026-10-18).
    For example, include each book's series in the book list the Kindle
    sends, so Calibre's device view shows it. Settle exactly what's wanted
    first: what Calibre does with series from the device, and whether that
    differs for books Calibre already has in its library. **Closed, nothing
    to do** (2026-10-18): the book list already echoes Calibre's metadata,
    series included, and Calibre shows a device book's series in its book
    details. The device view simply has no Series column (`DeviceBooksModel`
    in `gui2/library/models.py`). The developer had assumed we weren't
    sending it.

<a id="later-12"></a>
### Later 12

12. **Choose where books are stored** (developer, 2026-10-18): a setting
    for the folder under `/mnt/us/documents` that received books go into.
    Harder than the other settings in the app: what happens to books
    already received (move them, or leave them where they are and keep
    their paths), how lpaths (ADR 0003) and the Kindle's catalog entries
    follow a move, and what the app should offer as choices. Developer
    (2026-10-18): a folder picker rather than typing, and books already
    received stay where they are. Subfolders of `documents` are indexed on
    5.20.1 (research note, "Books in subfolders are indexed"), so the books
    root can stay `documents` with the folder as a path prefix, which
    Calibre accepts (`canSupportLpathChanges`). **Built and checked on the
    Kindle** (ADR 0023, 2026-10-19). Done.

<a id="later-13"></a>
### Later 13

13. **Fractional series positions** (developer, 2026-10-18): check that
    series sync handles them, for example Mistborn 3.5. The code passes
    Calibre's `series_index` through as `itemPosition` (3.5) with the label
    "3.5" (`series.rs`, `index_label`), but nobody has checked on the
    device whether the catalog keeps 3.5 and the Library orders and labels
    the book correctly. **Closed** (2026-10-18; research note, "Fractional
    series positions"): the catalog keeps 3.5 and orders the book right,
    but the Library's badge parses the label as a whole number, so it
    shows "3". Nothing we can send changes that.
