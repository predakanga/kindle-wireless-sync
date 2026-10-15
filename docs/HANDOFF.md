# Handoff (2026-10-15)

For the next session. Read `CLAUDE.md` first, then this, then `docs/PLAN.md`.

## Where we are

**Phases 1–3 are done. Phase 4 (on-device UI and KPM package) is about half
done.** The developer has since used it for real: they sent a book from
Calibre to the Kindle to read, and it worked perfectly.

Working on the reference Kindle (PW12, FW 5.20.1, SSH host `kindle`):
- An upstart service (`kindle-wireless-sync`, autostart on) that waits for
  Wi-Fi, connects only to allowed Calibre libraries, receives and deletes
  books into `/mnt/us/documents`, and writes covers before the book appears
  (ADR 0005). It reconnects after Wi-Fi drops, and stays disconnected after
  an eject until Wi-Fi cycles (ADR 0008).
- A localhost API on `127.0.0.1:8325` (ADR 0011) and the WAF settings app
  ("Wireless Sync" tile in the Library). Live status, Pause/Resume and
  opening from the tile are verified on the device.

Code: `src/` has `protocol`, `session`, `library`, `kindle` (covers/EXTH),
`lpath`, `discovery`, `config` (allowlist with flock), `daemon` (loop and
status), `network` (lipc Wi-Fi) and `api` (hand-written HTTP). There are 58
tests, including a replay of real Calibre sessions
(`tests/replay.rs`, anonymized fixture). Device-side files are in
`package/` (upstart jobs, `service.sh`, `autostart.sh`, `register-app.sh`,
`scriptlet.sh.in`, `ui/`, `icon/`).

## Next steps (in order)

1. ~~Finish checking the UI on the device~~ (done; see `docs/PLAN.md`).
   Possible polish: the status messages vanish after 6 s, which is easy to
   miss on e-ink.
2. **KPM package** (ADR 0004, 0010; see `docs/research/kindle-platform.md`):
   - `manifest.json` (manifest_version 2, id `kindle-wireless-sync`,
     `supported_platforms: ["kindlehf"]`), at the **root** of a gzip tarball
     named `.kpkg`.
   - `install.sh`, run by KPM from `/mnt/us/kmc/kpm/packages/<id>/`:
     - copy the binary (built **without** `dev-tools`: `just
       build-kindle-package`), `service.sh`, `autostart.sh`,
       `register-app.sh` and `ui/` to `/var/local/kindle-wireless-sync/`
     - install the two upstart files with `mntroot rw`/`ro` and a trap,
       only if their content changed, then `initctl reload-configuration`
     - `register-app.sh register`, put `WirelessSync.sh` in
       `/mnt/us/documents/`, and start the service
     - leave autostart **off** by default (ADR 0002: opt-in)
   - `uninstall.sh`: `exit 0` straight away when `$1 = upgrade`, to keep
     state. Otherwise stop the service, remove the upstart files (`mntroot`),
     `register-app.sh unregister`, remove the scriptlet and
     `/var/local/kindle-wireless-sync/`. **Leave received books and
     thumbnails** (they are the user's).
   - A `just package` recipe. Then test a real install, upgrade and
     uninstall with the device's KPM (`/var/local/kmc/bin/kpm`; check its
     CLI first).
3. Then Phase 5 stretch goals (`docs/PLAN.md`).

## Decisions (all in `docs/decisions/`, all Accepted)

0001 reference device / best-effort firmware · 0002 user start/stop and
optional autostart · 0003 Calibre trust model, allowlist by library UUID ·
0004 WAF app + localhost API, two upstart jobs, files in
`/var/local/kindle-wireless-sync` · 0005 register books via the scanner, cover
before rename · 0006 one crate, blocking I/O, `serde_json` without derives,
measure dependency size · 0007 accept azw3/mobi/azw/kfx/pdf/txt · 0008 daemon
behaviour; find libraries on request; synchronous add-server · 0009
`dev-tools` feature gates `discover`/`connect` · 0010 autostart sentinel and
crash-loop guard (credits usbnetlite, Button Mapper, HID Passthrough) · 0011
localhost API, Origin check, flock-guarded config, hand-written HTTP
(`tiny_http` measured at +143 KB).

## Working agreements with the developer

- Plan, then research, then build. Commit small and often. Gate commits on
  `just check && cargo test`, so a failure stops the commit.
- New ADRs are **Proposed** until the developer accepts them. Before changing
  an agreed design, present pros and cons with a recommendation and wait.
- Measure the size cost of any new dependency (armv7-musl release).
- Credit prior art precisely.
- Ask before writes to the Kindle's root filesystem, reboots, or anything
  else hard to undo. Name test files `kws-*` and clean up afterwards
  (including the `.sdr` folders the indexer creates).
- Never commit raw transcripts of the developer's library. Anonymize with
  `tools/anonymize_transcript.py`.

## Gotchas learned the hard way

- **zsh:** an argument starting with `=` (for example `echo =====`) aborts
  the whole command line ("== not found").
- **`pkill -f PATTERN` over SSH matches its own `sh -c` command line** and
  kills the SSH session. Use a self-excluding pattern:
  `pgrep -f "[k]indle-wireless-sync daemon"`, then `kill`.
- **Don't hide deploy output** (`| tail -1 >/dev/null`). It once hid that a
  stale binary was deployed.
- `cargo fmt` reformats code that Python search-and-replace edits rely on.
  Re-read before editing, and avoid line-range slicing (it dropped braces
  twice).
- **Calibre serves one device at a time.** While the Kindle is connected,
  probes from elsewhere (for example `find` on the Mac) get "busy". Calibre
  also refuses an IP after 5 incomplete handshakes until one succeeds.
- **Kindle:** Wi-Fi goes down with the screensaver. During long device tests
  use `lipc-set-prop -i com.lab126.powerd preventScreenSaver 1` (a reboot
  resets it). `cc.db`/`ksdk.asset.db` must never be written. UI checks use
  `/usr/sbin/screenshot` plus Button Mapper's `tap.sh X% Y%` (`76 96` is the
  Library tab).
- Calibre CLI tools are in `/Applications/calibre.app/Contents/MacOS/` (not
  on PATH). Test books are in `~/Calibre Library/`.

## Device state at handoff

Service running, autostart on, connected to the developer's Calibre
("Calibre Library", UUID `00000000-0000-4000-8000-000000000000`, allowed). The
deployed binary is a **dev-tools** build; the package build will replace it.
Books on the device that came through the client: a test book (from testing)
and the developer's own reading book. Screensaver is normal. No test
leftovers.
