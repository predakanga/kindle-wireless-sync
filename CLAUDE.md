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

Phase 2 of `docs/PLAN.md`: the Calibre protocol client runs on the host and
works against real Calibre. Kindle integration (Phase 3) hasn't started.
Research notes, including on-device probe results for FW 5.20.1, are in
`docs/research/`. Read them before making design changes.

## Commands

```bash
just test                 # all tests (cargo test)
just test-one <filter>    # tests matching a name, e.g. `just test-one lpath`
just check                # cargo fmt --check + clippy -D warnings
just discover             # list Calibre servers on the LAN
just connect --allow <library-uuid> [--transcript transcripts/x.jsonl]
                          # act as a device; books land in ./dev-device/
just size-kindle          # build the armv7 musl binary and print its size
```

In Calibre, run *Connect/share → Start wireless device connection* first.
`just connect` without `--allow` prints the library UUID to allow.

## Code layout

One package, `kindle-wireless-sync` (ADR 0006): the binary has that name and the library crate is `kws`. It uses blocking std I/O and
`serde_json::Value` without serde derives, to keep the binary small. Measure
the size cost of any new dependency.

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
- **Commit as you go**, in small commits at each meaningful step (a research
  finding written up, a decision recorded, a feature working). Don't batch a
  whole phase into one commit.

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
