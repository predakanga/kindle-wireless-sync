# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

A Rust homebrew package for jailbroken Kindles, installed through KPM. It
receives books from Calibre's **Wireless device connection** and adds them
straight to the Kindle's **built-in** reader. It does not target KOReader or
any other third-party reader.

Hard constraints (from the requirements in `docs/PLAN.md`):
- Rust only, packaged for KPM.
- The user's PC runs stock Calibre and nothing else. A Calibre plugin is
  allowed only for the stretch goals (Series→collections, progress/rating
  sync back to Calibre).
- An on-device UI lets the user link the Kindle to one Calibre library (ADR
  0018, replacing the allowlist) and start/stop the sync service, with optional autostart at boot or network-up (ADR 0002).
- The reference device is a Kindle Paperwhite 12th Gen on FW 5.20.1. Other
  devices and firmware are best-effort (ADR 0001).
- Books appear in the Kindle library as soon as a sync finishes.

## Current status

Phases 1–4 are done. **Start with `docs/PLAN.md`**: it has the phases, what
comes next, the "Later" list of deferred items and the open questions.
Decisions are in `docs/decisions/`. Research notes, including on-device
probe results for FW 5.20.1, are in `docs/research/`. Read them before
making design changes.

## Commands

```bash
just test                 # all tests (cargo test)
just test-one <filter>    # tests matching a name, e.g. `just test-one lpath`
just check                # fmt --check + clippy -D warnings, with and without dev-tools
just discover             # dev: list Calibre servers on the LAN
just connect --allow <library-uuid> [--transcript transcripts/x.jsonl]
                          # dev: act as a device on the host; books land in ./dev-device/
just deploy               # build (with dev-tools) and copy to the Kindle
just device-connect --allow <uuid>   # dev: one foreground session on the Kindle
just install-service      # install the upstart jobs (briefly remounts / read-write)
just start | stop | restart | status | logs | autostart-on | autostart-off
just screensaver-off | screensaver-on   # keep the Kindle awake for long tests (Wi-Fi drops with it)
just size-kindle          # size of the packaged (no dev-tools) armv7 binary
just deploy-ui | open-ui  # dev: copy the WAF app and scriptlet; open the app
just ui-prototype         # dev: open the clickable UI prototype (tools/ui-prototype) in a browser
just package              # build target/package/kindle-wireless-sync.kpkg (ADR 0012)
just kpm-install | kpm-uninstall   # install/remove the package with the device's own KPM
just series-grouping [status|enable|disable]   # dev: native series grouping on the Kindle (not packaged)
just calibre launch|status|wireless-start|wait-device|send|delete|field|device-books|dialogs|exec|quit ...
                          # dev: an isolated Calibre in dev-calibre/, driven by a plugin (ADR 0019)
just release X.Y.Z        # prepare a release: bump, date the changelog, commit (no push)
just publish-release [-y] # tag the prepared version and push main + tag (publishes; ask first)
```

**Releases (ADR 0014):** `just release X.Y.Z` (`tools/release.sh`) bumps
`Cargo.toml` and `Cargo.lock`, moves the `Unreleased` entries in
`CHANGELOG.md` under `## [X.Y.Z] - today`, updates the compare links and
commits. It refuses a dirty tree, a branch other than `main`, a version that
isn't newer, or an empty `Unreleased`. Show the developer the commit, then
(after asking) `just publish-release -y` tags `vX.Y.Z` and pushes `main` and
the tag atomically. The workflow fails if the changelog has no section for
the version, and uses that section as the release notes. `.github/workflows/release.yml` checks, tests and
packages the release, publishes it on GitHub, and rebuilds the KPM index on
GitHub Pages (`https://predakanga.github.io/kindle-wireless-sync/repo.json`).
Run the workflow by hand to rebuild only the Pages site.

**CI:** `.github/workflows/ci.yml` runs `just check`, `cargo test` and `just
package` (with the 5 MB size check) on pushes to `main` and on pull
requests. The tests need the `sqlite3` CLI.

**Calibre harness:** `tools/calibre-harness/README.md`. It needs the
developer's own Calibre closed, so ask them to quit it. When you use `just calibre exec`, add the
snippet to that README's "`exec` snippets in use" list with the date. Snippets
that recur become named commands.

`discover`, `connect` and `check` exist only with the `dev-tools` cargo
feature (ADR 0009). `check --state DIR HOST [PORT] [--password PW] [--quiet
SECS]` runs the app's library check (ADR 0020) from the Mac. The packaged
binary has `daemon`, `find`, `link`, `unlink`, `status` and `series`. In
Calibre, run *Connect/share → Start wireless device connection* first.

**The whole service runs on the Mac** with scratch folders: `daemon --books
DIR --state DIR`. Without lipc it assumes Wi-Fi is up. With the harness
Calibre, this tests the app (`package/ui/`) end to end in headless Chrome.
Load the app from a `file://` URL, or the API refuses it (ADR 0011), and
drive it in real time: `--virtual-time-budget` races ahead of real network
requests.

## Code layout

One package, `kindle-wireless-sync` (ADR 0006): the binary has that name and the library crate is `kws`. It uses blocking std I/O and
`serde_json::Value` without serde derives, to keep the binary small. The
size budget is loose: keep the packaged binary well under 5 MB (`just
size-kindle`), and check it when adding a dependency. There's no need to
track individual bytes.

- `protocol`: opcodes and the length-prefixed JSON framing. A `Connection`
  wraps a reader and writer and can record a JSONL transcript.
- `session::run`: the device side of a Calibre conversation. Calibre sends
  every request and we only reply. The reply shapes must match Calibre's
  `smart_device_app/driver.py` exactly. `research/calibre-wireless-protocol.md`
  has the walkthrough.
- `library`: the books directory plus our state (`books.json`,
  `driveinfo.json`). All book writes follow ADR 0005: a dot-prefixed temp
  file then rename for new books, an in-place rewrite for replacements, and
  deleting the `.sdr` folder along with a book.
- `lpath`: validation of server-supplied paths. Calibre is untrusted (ADR 0003).
- `discovery`: UDP broadcast to Calibre's fixed ports.
- `catalog` and `series`: Calibre series as native series (ADR 0016).
  `catalog` sends ChangeRequest commands (`127.0.0.1:9101`, token from
  `/tmp/session_token`) and reads `cc.db` read-only. `series` plans the
  commands that differ. `Library` runs a pass 15 s after book changes, on
  Calibre's keep-alive NOOPs or in the daemon's idle waits.
- `readsync`: read status and last-read date to Calibre (ADR 0017). The
  session names the sync columns in its handshake once `SET_LIBRARY_INFO`
  has shown they exist (kept with the linked library in `config.json`, ADR
  0020). When they change, it
  ends with `SessionEnd::ColumnsChanged` at the first keep-alive after 10 s
  of idleness, without replying to it (ADR 0022), and the daemon
  reconnects. Dropping the connection while
  Calibre is mid-job makes it show an error dialog.
  `Library::read_sync_fields` reads `cc.db` and adds `_is_read_` and
  `_last_read_date_` to the book list. Only when the linked library has
  `writable: true` in `config.json`.
- `config` and `daemon`: one linked library (ADR 0018). Found libraries
  live in the daemon's memory. Linking only changes `config.json`; the
  daemon resets the state (`Library::reset_if_relinked`) before its next
  session when driveinfo's `last_library_uuid` names another library.
- `adopt`: finds book files Calibre sent over USB and builds records for them
  from `metadata.calibre` or EXTH (ADR 0013). `Library::adopt_existing` runs
  it once per session.

`tests/replay.rs` replays anonymized recordings of real Calibre sessions from
`tests/fixtures/`. To add one, record with `just connect ... --transcript
transcripts/NAME.jsonl` (the folder is gitignored), then run
`tools/anonymize_transcript.py tests/fixtures/OUT.jsonl transcripts/A.jsonl
[...]`. Never commit raw transcripts: they hold the user's library metadata.

## Working process

- **Plan, then research, then build.** `docs/PLAN.md` holds the phases, exit
  criteria and open questions. Keep it current: tick off phases and move
  answered questions into decisions.
- **Record design decisions** as ADRs in `docs/decisions/` (template and index
  in its README). Write one whenever a choice affects architecture, the
  protocol, the on-device integration or the packaging. Cite evidence such as
  Calibre source paths or device observations.
- **Keep `CHANGELOG.md` current:** add user-facing changes under
  `## [Unreleased]` as you make them.
- **Licence headers:** every new source or script file starts with an
  `SPDX-License-Identifier: GPL-3.0-or-later` line in its comment syntax:
  `// ` for Rust, `# ` after the shebang for shell and Python, `/* */` for
  JS and CSS, and `<!-- -->` after `<!DOCTYPE>` for HTML. In the scriptlet
  it goes after the Name/Author/Icon header. Formats without comments
  (JSON) are exempt. The copyright line lives in the README.
- **Commit as you go**, in small commits at each meaningful step (a research
  finding written up, a decision recorded, a feature working). Don't batch a
  whole phase into one commit.
- **Don't push or tag without asking.** Commits stay local until the
  developer says to push. A `vX.Y.Z` tag publishes a release (ADR 0014), so
  only propose one for a real, user-facing change.

## Device access

- The reference Kindle is reachable as SSH host `kindle`, over USB networking
  (`usb0`, 192.168.15.244). Toggling Wi-Fi on the device does not drop the
  session.
- Never write to `/var/local/cc.db` or `/var/local/ksdk.asset.db`. The two
  stores are kept in sync by triggers, and hand edits can desync the library
  (ADR 0005). Reads must tolerate `database is locked`.
- To check the UI from SSH, run `/usr/sbin/screenshot`. It writes a PNG to
  `/mnt/us/screenshots/`; delete it after fetching. Button Mapper's
  `/mnt/us/kindle-button-mapper/scripts/tap.sh 76 96` taps the Library tab.
- Name test files `kws-*`. When cleaning up, also remove the `.sdr/` folder
  the indexer creates for each file, because it is not removed automatically.
- **Make test files TXT, never copies of real books.** A copy shares the
  original's EXTH key, and so its asset-store row.
- Ask the developer before writes to the root filesystem (`mntroot`),
  reboots, or anything else hard to undo.
- When you ask the developer to check something on the device, wait for
  their answer before the next step, and don't undo what they're checking.
- Wi-Fi goes down with the screensaver. For long device tests, run
  `just screensaver-off`, and `just screensaver-on` afterwards (a reboot also
  turns it back on).
- **Quitting Calibre ejects the Kindle.** The daemon then broadcasts until
  Calibre is back (ADR 0021), unless Calibre never answered a discovery
  datagram (a firewall): then it waits, as after a user's eject (ADR 0008). To reconnect from SSH, run `curl -s -X POST
  http://127.0.0.1:8325/connect-now` on the Kindle. Connect now does nothing
  while a session is open: for a fresh book list, run `just restart`.
- To run the daemon by hand (for example with `RUST_LOG=debug`), stop the
  service, then start the binary with the arguments from `service.sh`.
- **Closing the app doesn't stop it.** Its mesquite process keeps running
  in the background with the page it loaded, through ✕ and app switches,
  until a reboot. `just deploy-ui` and `install.sh` stop it with
  `lipc-set-prop com.lab126.appmgrd stop app://com.kindlewirelesssync.app`;
  killing mesquite instead makes appmgrd report a crash. appmgrd starts a
  fresh process at once, but it reads the files only when the app opens,
  so stopping before copying works. If the app shows old wording after a
  deploy, that's why (2026-10-18).
- **Library menus change size.** A book's ⋮ menu gains items ("Search this
  book" appears once the book has been opened), so "Mark as read/unread"
  moves. Take a screenshot before tapping. The first tile's ⋮ button is at
  about (29%, 51%).
- The device has no `python3`. KPM is `/var/local/kmc/bin/kpm` (0.2.2).
  Its source is github.com/KindleModding/KPM (0.2.2 is `07495e0` to
  `94608b3^`). github.com/gingrspacecadet/kpm is a different tool with the
  same name. ADR 0012 records how KPM runs our hooks. A local `.kpkg` can
  only be installed fresh: to test an upgrade, serve it from a temporary
  `file://` `repo.json` (`kpm add-repo`, `kpm update`, `kpm install`), and
  remove the repo afterwards.

### Emulated Kindle

`../eink-emulator` runs Kindle firmware in QEMU; read its `AGENTS.md`
first. Instance `pw6-5201-jb` matches the reference device: a Paperwhite
12th Gen (`kindle-paperwhite-6`) on 5.20.1, jailbroken with `--jbsh`, so
KPM is present. The developer's own `pw12` instance is theirs; leave it
alone.

**When the emulator misbehaves** (Wi-Fi stuck at PENDING, DNS failures,
timeouts, odd network results), check these two first and tell the
developer what you find. Either one alone breaks QEMU's DNS proxies on the
developer's Mac (2026-10-17):
1. **Is the real Kindle plugged in?** `ifconfig | grep 'inet 192.168.15'`
   on the Mac. Its USB network overlaps the emulator's 192.168.15.0/24, and
   the Wi-Fi-side proxy (10.0.2.3) times out.
2. **Is Tailscale managing DNS?** `grep nameserver /etc/resolv.conf` shows
   `100.100.100.100`. Both proxies fail at once.

Confirm with `nslookup example.com 10.0.2.3` (and `192.168.15.3`) in the
guest, compared with `1.1.1.1`. Ask the developer to unplug the Kindle or
quit Tailscale; don't change either yourself.

- Start it in the background from `../eink-emulator`:
  `./eink run pw6-5201-jb --headless --serial-socket --qmp-socket
  --ssh-port 4022 --wifi-ssh-port 4023`. SSH host `kindle-emu`; every
  device recipe takes it, e.g. `KINDLE_HOST=kindle-emu just kpm-install`.
  Its SSH server (the emulator's dropbear) runs with umask `0077`, unlike
  the firmware's `002`. Files created over it are private to root: mind
  that for anything the framework user must read (ADR 0012).
  The instance's install dates from before `install.sh` set modes itself
  (2026-10-17), so its app was `0700` and wouldn't open ("The selected
  application could not be started"). `just deploy-ui` repaired it
  (2026-10-18); a fresh `kpm install` would too.
- Home is up when `lipc-get-prop com.lab126.appmgrd activeApp` says
  `com.lab126.KPPMainApp` (Home and Library both live in it). Check with
  `python3 scripts/eink-qmp.py --machine pw6-5201-jb screendump F.png`; a
  capture can land mid-refresh (a dithered screen), so take another.
- `eink-qmp.py tap X Y` takes screen pixels (1272×1696). In a screenshot
  scaled to height H, multiply by 1696/H. On the on-screen keyboard, "123"
  switches to digits, and Enter is at the bottom right.
- Join Wi-Fi over SSH: `echo '{ essid = "Kindle-QEMU", secured = "no" }' |
  lipc-hash-prop com.lab126.wifid createProfile`, then `lipc-set-prop
  com.lab126.wifid cmConnect Kindle-QEMU`. `wifid` reaches CONNECTED only
  once its DNS probe passes, and the daemon waits for CONNECTED.
- The guest reaches the Mac at 10.0.2.2 over Wi-Fi and 192.168.15.201 over
  USB. UDP broadcast doesn't cross QEMU's network, so plain `find` can't
  see Calibre, but a datagram sent to 10.0.2.2 gets its answer
  (2026-10-18): `find --probe 10.0.2.2` works, and so does ADR 0021. Link
  by address, with the app, `/link` or `link`.
- `ping` in the guest is unreliable on macOS: replies arrive late and with
  "wrong data byte" errors. Don't read anything into it.

## Gotchas

- **zsh:** an argument starting with `=` (for example `echo =====`) aborts
  the whole command line. `$var:x` is a zsh modifier, so write `${var}:x`.
- **`pkill -f PATTERN` over SSH matches its own `sh -c` command line** and
  kills the session. Use a self-excluding pattern:
  `pgrep -f "[k]indle-wireless-sync daemon"`, then `kill`. Never kill and
  start the daemon in one SSH command: the new command line contains
  "kindle-wireless-sync daemon", so the pattern matches the shell. Use two
  SSH calls.
- `RUST_LOG=debug` truncates long protocol messages with "…".
- **Calibre swallows sync errors.** `set_books_in_library` wraps
  `synchronize_with_db` in `except Exception: return True`, so a book can be
  skipped with nothing logged.
- **Don't hide deploy output** (`| tail -1 >/dev/null`). It once hid that a
  stale binary was deployed.
- `cargo fmt` reformats code that search-and-replace edits rely on. Re-read
  before editing, and avoid line-range slicing.
- `just deploy` (with dev-tools) and `just package` (without) build to the
  same target path. The last one built wins.
- KPM reports network failures only as a number: `(4)` is `KPM_CURL_ERROR`,
  which usually means Wi-Fi is down (screensaver).
- **Calibre serves one device at a time.** While the Kindle is connected,
  probes from elsewhere get "busy". Calibre also refuses an IP after 5
  incomplete handshakes until one succeeds.
- Calibre CLI tools are in `/Applications/calibre.app/Contents/MacOS/` (not
  on PATH). Test books are in `~/Calibre Library/`.
- **The Kindle's busybox `sleep` takes whole seconds only** (`sleep 0.5`
  fails). zsh doesn't split unquoted variables, so `t $xy` passes one
  argument: write coordinates out.
- **`scp` to the Kindle once failed repeatedly** ("Connection closed by
  remote host") while plain SSH worked. `ssh kindle 'cat > FILE' < FILE`
  is a reliable fallback.
- **Testing a KPM upgrade needs a higher version** than the device has,
  including earlier test builds (the developer's Kindle reached 0.2.9 that
  way). Bump `Cargo.toml` only for the package build, then revert it.

## Debugging Calibre

For the isolated test Calibre, prefer the harness
(`tools/calibre-harness/README.md`). For the developer's own Calibre:
- Ask the developer to quit Calibre, then run
  `/Applications/calibre.app/Contents/MacOS/calibre-debug -g` in the
  background, with output to a file in the scratchpad.
- The driver's `SMART_DEV` lines reach the file only when Calibre exits
  (`PYTHONUNBUFFERED` doesn't help). Quit it with `osascript -e 'quit app
  "calibre"'` (it reports "User cancelled" but quits), then read the log.
- Calibre's per-device metadata cache is
  `~/Library/Caches/calibre/wireless_device_<device_store_uuid>_metadata_cache.json`:
  length-prefixed JSON records (an 8-byte length line, then the record).
- Read Calibre's database read-only: `sqlite3 -readonly "$HOME/Calibre
  Library/metadata.db"`. Custom columns are listed in `custom_columns`, with
  values in `custom_column_N`.

## Key references

- Calibre smart device driver (the server side of the wireless protocol):
  `src/calibre/devices/smart_device_app/` in https://github.com/kovidgoyal/calibre
- KOReader's `calibre.koplugin`, an existing client implementation of the
  same protocol: https://github.com/koreader/koreader/tree/master/plugins/calibre.koplugin
- Kindle Button Mapper, the model for the Rust Kindle GUI app, KPM packaging
  and the justfile debug/deploy workflow:
  https://github.com/zampierilucas/kindle-button-mapper-rs
- Kindle HID Passthrough, the model for a user-controlled service that can
  start at boot or network-up:
  https://github.com/zampierilucas/kindle-hid-passthrough
