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
- An on-device UI lets the user allowlist Calibre connections and start/stop
  the sync service, with optional autostart at boot or network-up (ADR 0002).
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
just start | stop | status | logs | autostart-on | autostart-off
just size-kindle          # size of the packaged (no dev-tools) armv7 binary
just deploy-ui | open-ui  # dev: copy the WAF app and scriptlet; open the app
just package              # build target/package/kindle-wireless-sync.kpkg (ADR 0012)
just kpm-install | kpm-uninstall   # install/remove the package with the device's own KPM
just series-grouping [status|enable|disable]   # dev: native series grouping on the Kindle (not packaged)
```

**Releases (ADR 0014):** bump `version` in `Cargo.toml`, move the
`Unreleased` entries in `CHANGELOG.md` under a new `## [X.Y.Z] - date`
heading, commit, then (after asking) push a matching `vX.Y.Z` tag. The
workflow fails if the changelog has no section for the version, and uses
that section as the release notes. `.github/workflows/release.yml` checks, tests and
packages the release, publishes it on GitHub, and rebuilds the KPM index on
GitHub Pages (`https://predakanga.github.io/kindle-wireless-sync/repo.json`).
Run the workflow by hand to rebuild only the Pages site.

**CI:** `.github/workflows/ci.yml` runs `just check`, `cargo test` and `just
package` (with the 5 MB size check) on pushes to `main` and on pull
requests. The tests need the `sqlite3` CLI.

`discover` and `connect` exist only with the `dev-tools` cargo feature (ADR
0009). The packaged binary has `daemon`, `find`, `add-server`, `allow`
and `status`. In Calibre, run *Connect/share → Start wireless device
connection* first.

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
- Ask the developer before writes to the root filesystem (`mntroot`),
  reboots, or anything else hard to undo.
- When you ask the developer to check something on the device, wait for
  their answer before the next step, and don't undo what they're checking.
- Wi-Fi goes down with the screensaver. For long device tests, run
  `lipc-set-prop -i com.lab126.powerd preventScreenSaver 1`, and set it back
  to 0 afterwards (a reboot also resets it).
- The device has no `python3`. KPM is `/var/local/kmc/bin/kpm` (0.2.2).
  Its source is github.com/KindleModding/KPM (0.2.2 is `07495e0` to
  `94608b3^`). github.com/gingrspacecadet/kpm is a different tool with the
  same name. ADR 0012 records how KPM runs our hooks. A local `.kpkg` can
  only be installed fresh: to test an upgrade, serve it from a temporary
  `file://` `repo.json` (`kpm add-repo`, `kpm update`, `kpm install`), and
  remove the repo afterwards.

## Gotchas

- **zsh:** an argument starting with `=` (for example `echo =====`) aborts
  the whole command line. `$var:x` is a zsh modifier, so write `${var}:x`.
- **`pkill -f PATTERN` over SSH matches its own `sh -c` command line** and
  kills the session. Use a self-excluding pattern:
  `pgrep -f "[k]indle-wireless-sync daemon"`, then `kill`.
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
