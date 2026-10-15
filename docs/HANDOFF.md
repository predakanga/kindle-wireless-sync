# Handoff (2026-10-15, second session)

For the next session. Read `CLAUDE.md` first, then this, then `docs/PLAN.md`.

## Where we are

**Phases 1–4 are done.** The Phase 4 exit criterion was met on the
device: a fresh KPM install, then Find libraries, Allow, and a book from
stock Calibre, which appeared in the library.

Working on the reference Kindle (PW12, FW 5.20.1, SSH host `kindle`):
- **The KPM package (ADR 0012), installed through the device's KPM 0.2.2.**
  It has the upstart service (ADR 0004, 0010), the localhost API on
  `127.0.0.1:8325` (ADR 0011), and the WAF settings app with its "Wireless
  Sync" Library tile.
- **The UI has been fully checked on the device:**
  - the keyboard in the Add server and password fields
  - Find libraries and Allow
  - Add server, with the error for a bad address
  - the password form, with save, clear and cancel
  - Forget (two taps)
  - the autostart toggle
  - Pause/Resume and live status

Code: `src/` has `protocol`, `session`, `library`, `kindle` (covers/EXTH),
`lpath`, `discovery`, `config`, `daemon`, `network` and `api`. There are 59
tests. Device-side files, including the KPM hooks, are in `package/`.
`just package` builds `target/package/kindle-wireless-sync.kpkg`.
`just kpm-install` and `just kpm-uninstall` drive the device's own KPM.

## Next steps (in order)

1. **The developer to review ADR 0012 (KPM package) and ADR 0013 (adopting
   USB-sent books).** Both are Proposed.
2. **Build ADR 0013** once it's accepted. Then check the two "To verify"
   items on the device:
   - whether Calibre takes a USB connection while our wireless session is
     open
   - whether a minimal EXTH-only record is accepted on a cache miss

   USB-sent books for testing are already on the device: an AZW3
   (listed in `/mnt/us/metadata.calibre`), and EPUBs that
   must be skipped. Measure the binary size before and after.
3. **Distribution** (open question in `docs/PLAN.md`). KPM 0.2.2 can't
   install a local `.kpkg` over an installed package (ADR 0012, findings),
   so upgrades need a KPM repository (`repo.json`). Ask the developer where
   to host it, and whether to report the bug upstream (KindleModding/KPM).
   Don't post anything without asking.
4. Possible polish: status messages vanish after 6 s, which is easy to miss
   on e-ink.
5. Phase 5 stretch goals (`docs/PLAN.md`).

## Decisions (all in `docs/decisions/`)

Accepted: 0001 reference device / best-effort firmware · 0002 user
start/stop and optional autostart · 0003 Calibre trust model, allowlist by
library UUID · 0004 WAF app + localhost API, two upstart jobs, files in
`/var/local/kindle-wireless-sync` · 0005 register books via the scanner,
cover before rename · 0006 one crate, blocking I/O, `serde_json` without
derives, measure dependency size · 0007 accept azw3/mobi/azw/kfx/pdf/txt ·
0008 daemon behaviour; find libraries on request; synchronous add-server ·
0009 `dev-tools` feature gates `discover`/`connect` · 0010 autostart
sentinel and crash-loop guard · 0011 localhost API, Origin check,
flock-guarded config, hand-written HTTP.

**Proposed:** 0012 KPM package lifecycle (install is safe to re-run; upgrade
keeps state; uninstall keeps books, removes everything else) · 0013 adopt
USB-sent books (Calibre uuid only, `metadata.calibre` then EXTH, scan at
session start, EPUBs skipped).

## KPM facts (from its source)

The source is github.com/KindleModding/KPM. Version 0.2.2 is commits
`07495e0`–`94608b3^`. (github.com/gingrspacecadet/kpm is a different tool
with the same name.) ADR 0012 has the details.
- `install.sh` runs from the package directory. A non-zero exit deletes the
  package directory and leaves no record.
- An upgrade runs `uninstall.sh upgrade` first.
- `kpm install file:///x.kpkg` works for a fresh install only.
- A temporary local repository works for testing upgrades:
  `kpm add-repo file:///tmp/…/repo.json`, then `kpm update`, then
  `kpm install <id>`. Remove it afterwards.

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
- **When you ask the developer to check something on the device, wait for
  their answer before the next step.** Don't undo the thing they are
  checking.
- Never commit raw transcripts of the developer's library. Anonymize with
  `tools/anonymize_transcript.py`.

## Gotchas learned the hard way

- **zsh:** an argument starting with `=` (for example `echo =====`) aborts
  the whole command line ("== not found"). Also, `$var:x` is a zsh modifier,
  so write `${var}:x` (for example in `git show "${c}:path"`).
- **`pkill -f PATTERN` over SSH matches its own `sh -c` command line** and
  kills the SSH session. Use a self-excluding pattern:
  `pgrep -f "[k]indle-wireless-sync daemon"`, then `kill`.
- **Don't hide deploy output** (`| tail -1 >/dev/null`). It once hid that a
  stale binary was deployed.
- `cargo fmt` reformats code that Python search-and-replace edits rely on.
  Re-read before editing, and avoid line-range slicing (it dropped braces
  twice).
- `just deploy` (dev-tools build) and `just package` (no dev-tools) build to
  the same target path. The last one built wins.
- **Calibre serves one device at a time.** While the Kindle is connected,
  probes from elsewhere (for example `find` on the Mac) get "busy". Calibre
  also refuses an IP after 5 incomplete handshakes until one succeeds.
- **Kindle:**
  - Wi-Fi goes down with the screensaver. During long device tests use
    `lipc-set-prop -i com.lab126.powerd preventScreenSaver 1` (a reboot
    resets it), and set it back to 0 afterwards.
  - `cc.db`/`ksdk.asset.db` must never be written.
  - UI checks use `/usr/sbin/screenshot` plus Button Mapper's
    `tap.sh X% Y%` (`76 96` is the Library tab).
  - The scanner removed the tile's catalog entry 10–15 s after an
    uninstall.
  - The device has no `python3`.
- Calibre CLI tools are in `/Applications/calibre.app/Contents/MacOS/` (not
  on PATH). Test books are in `~/Calibre Library/`.

## Device state at handoff

- **Package:** `kindle-wireless-sync` 0.1.0, installed by KPM from a local
  file (repository `""` in `kpm.db`). The binary is the packaged build,
  without dev-tools.
- **Service:** running and connected to the developer's Calibre ("Calibre
  Library", UUID `00000000-0000-4000-8000-000000000000`, allowed).
- **Autostart:** **off**, as on any fresh install. The developer may turn it
  on in the app.
- **`books.json`:** tracks three books received over Wi-Fi.
  `driveinfo.json` keeps the original `device_store_uuid`
  (unchanged), restored after the reinstall so that Calibre sees the same
  device.
- **Other:** the screensaver is normal, and there are no test leftovers.
