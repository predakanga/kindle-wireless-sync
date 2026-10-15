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

## References

- Calibre source: https://github.com/kovidgoyal/calibre/tree/master
  (smart device driver: `src/calibre/devices/smart_device_app/`)
- Kindle Button Mapper: https://github.com/zampierilucas/kindle-button-mapper-rs
  (shows a Rust Kindle GUI app, KPM packaging and a justfile debug workflow)
- KOReader Calibre plugin: https://github.com/koreader/koreader/tree/master/plugins/calibre.koplugin
  (an existing client implementation of the wireless protocol)

## Phases

Each phase ends with its findings or decisions written down (see
[decisions/](decisions/)) and a commit. Later phases may change based on what
Phase 1 finds.

### Phase 0: Project setup ✅

- Repo, CLAUDE.md, this plan, decision log.

### Phase 1: Research and feasibility (no product code)

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
   Kindle UI. Also learn how to run a background service or on-demand
   listener, and how Wi-Fi and sleep behaviour affect a long-lived connection.

**Exit criteria:** a written answer to each item, a feasibility verdict for
every base requirement, and a first set of architecture decisions.

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

### Phase 3: Running on the Kindle

- Cross-compile and set up the build and deploy workflow (a justfile modelled
  on Kindle Button Mapper).
- Run the client on the device, write to the correct documents location, and
  trigger library registration so books show up immediately.
- Decide on the lifecycle: always-on service, start from the UI, or start when
  Wi-Fi comes up.

**Exit criteria:** a book sent from Calibre opens in the built-in reader
without a reboot.

### Phase 4: UI and packaging

- On-device UI for discovered Calibre instances and the allowlist (with
  password if one is needed), plus connection status and sync status.
- KPM package with install, upgrade and uninstall that leaves the device
  clean.

**Exit criteria:** a fresh install from KPM, through allowlisting, to a first
sync works end to end with only stock Calibre.

### Phase 5: Stretch goals

- Collections from Series.
- Progress and rating sync back to Calibre, with a Calibre plugin only if
  Phase 1 shows it is required.

## Open questions

Track these here until they become decisions.

- Which firmware versions and Kindle models do we support?
- What does "allowlist" mean in protocol terms? (It depends on who initiates
  the connection and what identifies a Calibre instance, such as library UUID,
  hostname or IP.)
- When does the client run (always-on vs on-demand), and what does that cost
  in battery?
