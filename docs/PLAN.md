# Project Plan

A KPM package for jailbroken Kindles that receives books from Calibre's
**Wireless device connection** (the "smart device" driver) and adds them to
the Kindle's **built-in** reader.

## Requirements

Base:

1. Written in Rust.
2. Ships as a package that KPM (Kindle Package Manager) can install.
3. Has an on-device UI where the user links the Kindle to a Calibre library
   (ADR 0018; an allowlist until then).
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

## Status at a glance

| | Status | Summary |
|---|---|---|
| Phase 0: Project setup | ✅ | Repo, CLAUDE.md, plan, decision log |
| Phase 1: Research and feasibility | ✅ | Every base requirement feasible with stock Calibre (ADRs 0001–0005) |
| Phase 2: Protocol client on the host | ✅ | Calibre treats the host CLI as a device (2026-10-15) |
| Phase 3: Running on the Kindle | ✅ | A sent book opens in the built-in reader without a reboot (2026-10-15) |
| Phase 4: UI and packaging | ✅ | App, KPM package and releases; latest **0.4.0** (2026-10-19) |
| Phase 5: Stretch goals | ✅ | Native series, read status to Calibre, device name; percent read deferred (Later 5) |
| Phase 6: Other firmware | Open | Check other firmware and models |
| Later 1–7, 9 | Open | See [Later](#later-to-revisit-once-the-goals-above-are-met) |
| Later 8, 10–13 | ✅ Done or closed | See [Later](#later-to-revisit-once-the-goals-above-are-met) |

Full status notes, as they were written, are in the
[progress log](progress-log.md).

## Phases

Each phase ends with its findings or decisions written down (see
[decisions/](decisions/)) and a commit. Later phases may change based on what
Phase 1 finds.

### Phase 0: Project setup ✅

- Repo, CLAUDE.md, this plan, decision log.

### Phase 1: Research and feasibility (no product code) ✅

The aim was to answer the open questions and confirm that every base
requirement can be met without a Calibre plugin. It covered the Calibre
wireless protocol, formats, instant availability, collections, progress and
ratings, and the Kindle platform.

**Exit criteria:** a written answer to each item, a feasibility verdict for
every base requirement, and a first set of architecture decisions.

**Done (2026-10-15):** research in [research/](research/), probed on 5.20.1;
ADRs 0001–0005 accepted. The research items and the feasibility table are in
the [progress log](progress-log.md#phase-1).

### Phase 2: Protocol client on the host ✅

The Calibre wireless client as a Rust library and CLI on the dev machine,
tested against a real Calibre with the Kindle's storage simulated locally.

**Exit criteria:** Calibre treats the host CLI as a connected device and can
send books to it and delete books from it.

**Done (2026-10-15)** against Calibre 9.16.0. Details:
[progress log](progress-log.md#phase-2).

### Phase 3: Running on the Kindle ✅

Cross-compiling, the deploy workflow, the daemon and its upstart jobs, and
library registration on the device.

**Exit criteria:** a book sent from Calibre opens in the built-in reader
without a reboot.

**Done (2026-10-15).** Details: [progress log](progress-log.md#phase-3).

### Phase 4: UI and packaging ✅

The on-device app, the KPM package, and releases through GitHub and a KPM
index on GitHub Pages (ADR 0014).

**Exit criteria:** a fresh install from KPM, through linking a library, to a
first sync works end to end with only stock Calibre.

**Done (2026-10-15)**, and extended since. Releases:

- **0.1.0, 0.1.1** (2026-10-15/16): first releases; device name (ADR 0015).
- **0.2.0** (2026-10-16): native series (ADR 0016).
- **0.3.0** (2026-10-18): one linked library (ADR 0018), the one-screen app
  and library checks (ADR 0020), read sync (ADR 0017).
- **0.3.1** (2026-10-18): deleted books leave the Library at once (ADR 0005).
- **0.3.2** (2026-10-18): reconnect after Calibre restarts (ADR 0021);
  leaving at a keep-alive and a graceful Disconnect (ADR 0022).
- **0.4.0** (2026-10-19): the storage folder (ADR 0023).

Details, including USB adoption (ADR 0013) and the UI review: [progress
log](progress-log.md#phase-4).

### Phase 5: Stretch goals ✅

- ✅ **Series as the Kindle's native series grouping** (ADR 0016, 0.2.0).
  Collections in general are Later 3.
- ✅ **Read status and last-read date to Calibre** (ADR 0017, 0.3.0).
  Percent read needs a Calibre plugin (Later 5); the other direction is
  Later 6. **Ratings are out of scope:** the device stores none for
  sideloaded books.
- ✅ **Kindle device name as the Calibre device name** (ADR 0015, 0.1.1).

Details: [progress log](progress-log.md#phase-5).

### Phase 6: Other firmware (stretch stretch goal)

- Check whether the on-device integration points we rely on (catalog
  registration, collections, progress storage, service hooks, UI) exist on
  firmware and models other than the reference device. Record what differs.
  See [0001](decisions/0001-target-device-and-firmware.md).
- **`just compat MODEL VERSION`** (`tools/compat/`) boots the model and
  firmware on eink-emulator and runs every integration check: architecture,
  commands, lipc, catalog and app-registry schemas, ChangeRequest, KPM
  install and uninstall, the service, indexing, series, Wi-Fi events, the
  app, and whether each page fits the screen. It prints a JSON line per run.
  Results so far (2026-10-19): Paperwhite 6 on 5.20.1 passes everything; a
  Basic 2016 on 5.16.2.1.1 fails, as a soft-float device with sqlite3
  3.26 (no `-json`). One open oddity there: catalog errors after a KPM
  install. [Details](progress-log.md#phase-6).
- Specific checks found so far:
  - **Books in subfolders of `documents` are indexed** like books in
    `documents` itself. The books folder (ADR 0023) depends on it, and it's
    only been checked on 5.20.1 (developer, 2026-10-19; research note,
    "Books in subfolders are indexed").

## Later: to revisit once the goals above are met

Not part of any phase. Pick these up after the base and stretch goals
(Phases 5–6) are done. Numbers stay fixed, since ADRs and commits refer to
them. Each item's full text is in the
[progress log](progress-log.md#later-items).

### Open

- **1. KPM bug: a local `.kpkg` can't replace an installed package.**
  Report it upstream, and decide how to ship upgrades until it's fixed
  (ADR 0012). [More](progress-log.md#later-1)

- **2. Review the UI.** E-ink usability and the wording of every message;
  maybe a translation file. Known: status messages vanish after 6 s, and
  Home's status with its two-line detail is three lines.
  [More](progress-log.md#later-2)

- **3. Kindle collections from Calibre fields** (series, tags, custom
  columns), through ChangeRequest. First check whether cloud collections
  sync to other devices. [More](progress-log.md#later-3)

- **4. A script that turns series grouping on or off,** packaged but not in
  the UI, asking for the device's UUID as confirmation. Starts from
  `tools/series-grouping.sh`. [More](progress-log.md#later-4)

- **5. A Calibre plugin for percent read,** extending the wireless
  connection rather than adding a second one. [More](progress-log.md#later-5)

- **6. Send read status from Calibre to the Kindle.** ChangeRequest can set
  it; first check whether that is reported to Amazon.
  [More](progress-log.md#later-6)

- **7. A tap-debugging helper in this repo,** instead of relying on Kindle
  Button Mapper's `tap.sh`. [More](progress-log.md#later-7)

- **9. Test KFX.** Receiving works; still to check deleting and replacing a
  KFX book, whether it carries Calibre's UUID, and relinking.
  [More](progress-log.md#later-9)

### Done or closed

- ✅ **8. Reconnect after Calibre restarts** (ADR 0021, 0.3.2).
  [More](progress-log.md#later-8)

- ✅ **10. End connections more reliably** (ADR 0022, 0.3.2).
  [More](progress-log.md#later-10)

- ✅ **11. Present the Series field back to Calibre:** closed, nothing to
  do; Calibre already gets it. [More](progress-log.md#later-11)

- ✅ **12. Choose where books are stored** (ADR 0023, 0.4.0).
  [More](progress-log.md#later-12)

- ✅ **13. Fractional series positions:** closed; ordered right, but the
  Library's badge shows only whole numbers. [More](progress-log.md#later-13)

## Open questions

Track these here until they become decisions.

- Should we also *write* the USB driver's `metadata.calibre`, so that USB
  connections see books we received over Wi-Fi? We keep our own state in
  `books.json`/`driveinfo.json`, and since ADR 0013 we only *read*
  `metadata.calibre` to adopt USB-sent books. Calibre's USB driver also
  reads metadata from files it doesn't know, so this may not be needed.
- What does a running service cost in battery, and does it stop the device
  from sleeping?
