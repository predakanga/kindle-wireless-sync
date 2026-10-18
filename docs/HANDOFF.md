<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Handoff (2026-10-18)

One-off notes for the next session. Read them, fold anything durable into
`CLAUDE.md`, `docs/PLAN.md` or an ADR, then delete this file (as with the
previous handoff).

## Where things stand
- **Released:** v0.3.0 and v0.3.1, both 2026-10-18, published through
  `just publish-release` (ADR 0014). The KPM index lists 0.3.1. The
  developer's Kindle runs 0.3.1, installed with `kpm update` and
  `kpm install`.
- **Unpushed local commits** after `v0.3.1`: `348623a` (PLAN: the releases
  recorded) and `5e679c2` (Home: "no columns have been chosen"). Ask before
  pushing; they can go out with the next release.
- **What 0.3.x contains:** the one-screen-per-page app and checking a
  library before linking it (ADR 0020), one linked library (ADR 0018), read
  sync with chosen columns (ADR 0017, amended), KFX keys and covers after
  indexing (ADR 0016, amended), a library switch in Calibre ending the
  session (ADR 0018, last section), and deleting through the catalog with
  covers kept until each entry is gone (ADR 0005, last section).

## Waiting on the developer
- **ADR 0021, Proposed:** telling Calibre quitting from an eject, by
  broadcasting Find's datagram at Calibre's address after an eject, so the
  Kindle reconnects by itself when Calibre comes back. It also keeps the
  Find cache in step with a changed port. The developer said to come back
  to it after the release. Build it once accepted; it answers Later item 8.

## New on the Later list (`docs/PLAN.md`)
- 10: end connections more reliably than waiting for a quiet keep-alive.
- 11: present the Series field back to Calibre (settle what's wanted first).
- 12: choose where books are stored (a harder UI question).

## Useful from this session
- **The dev-only `check` command** runs the app's library check from the
  Mac: `target/debug/kindle-wireless-sync check --state DIR HOST [PORT]
  [--password PW] [--quiet SECS]` (build with `--features dev-tools`).
- **The harness Calibre** (`dev-calibre/`, gitignored) now has two custom
  columns, "Kindle read" (Yes/No) and "Kindle last read" (Date), and a
  second library, `dev-calibre/library2`, for library-switch tests. The
  switch snippet is in `tools/calibre-harness/README.md`.
- **The whole service runs on the Mac** with scratch folders
  (`kindle-wireless-sync daemon --books DIR --state DIR`): no lipc means
  Wi-Fi is assumed up. With the harness Calibre this tests the app end to
  end in headless Chrome. Load the app from a `file://` URL so the API
  accepts it, and drive it in real time: `--virtual-time-budget` races
  ahead of real network requests.
- **On the emulated Kindle,** `eink-qmp.py tap X Y` takes screen pixels
  (1272×1696). In a screenshot scaled to height H, multiply by 1696/H. On
  the on-screen keyboard, "123" switches to digits, and Enter sits at the
  bottom right.
- **Kindle facts measured** (research notes): indexing takes about 15 s per
  file, queued one at a time; the scanner removes deleted entries about
  17 s after, then one every 3 to 4 s; `com.lab126.ccat` sends `updated`
  as the catalog changes; ChangeRequest `{"delete": {"uuid": ...}}` removes
  a book's entry from both stores at once.
- **Test files on the Kindle:** use TXT, never copies of real books. A copy
  shares the original's EXTH key, and so its asset-store row.
