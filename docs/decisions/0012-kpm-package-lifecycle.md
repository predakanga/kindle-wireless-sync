# 0012: KPM package install, upgrade and uninstall

- Status: Proposed
- Date: 2026-10-15

## Context
ADR 0004 fixes the layout: files in `/var/local/kindle-wireless-sync/`, two
upstart jobs installed only when their content changes, the WAF app
registered in `appreg.db`, and a scriptlet in `documents/`. ADR 0002 makes
autostart opt-in. This ADR records how the KPM hooks carry that out. It
follows how KPM itself behaves.

The device runs KPM 0.2.2 from the Kindle Modding Club
(github.com/KindleModding/KPM, commits `07495e0` to `94608b3^`; `src/install.c`,
`src/uninstall.c`, `cli/main.c`):
- **Install:** KPM deletes `/mnt/us/kmc/kpm/packages/<id>/`, extracts the
  `.kpkg` there, and runs `sh install.sh` from that directory. If the script
  exits non-zero, KPM deletes the directory and doesn't record the package,
  but nothing undoes what the script already did.
- **Reinstall or upgrade:** KPM first runs the old package's
  `sh uninstall.sh upgrade`. A failure there is only a warning. It then
  deletes the old directory and installs as above.
- **Uninstall:** KPM runs `sh uninstall.sh`. If it exits non-zero, KPM keeps
  the files and the database row, so the user can retry.
- **Local files:** `kpm install file:///path/x.kpkg` reads the manifest from
  the file. It skips the repository index, and with it the
  `supported_platforms` check, which only applies to indexed artifacts.
- **Launch:** `kpm launch <id>` runs the package's `launch.sh`, if it has one.
- After every install or uninstall, KPM runs `lipc-set-prop com.lab126.scanner doFullScan 1`.

On the device, the indexer creates `WirelessSync.sh.sdr/` (which holds the
tile icon) next to the scriptlet.

Kindle HID Passthrough's `scripts/install.sh` (3.17.5, installed on the
reference device) and Kindle Button Mapper's `install.sh`/`uninstall.sh`
show the same steps: stop the daemon, copy, register the app, `mntroot` with
a trap, and `initctl reload-configuration` after changing a job file.
HID Passthrough unloads its app with `lipc-set-prop com.lab126.appmgrd stop`
rather than `pkill`, because `pkill` makes appmgrd report a crash.

## Decision
**Package contents** (`.kpkg`, gzip tar, every name at the archive root):
`manifest.json`, `install.sh`, `uninstall.sh`, `launch.sh`, the binary
built **without** `dev-tools`, `service.sh`, `autostart.sh`,
`register-app.sh`, `WirelessSync.sh` (the scriptlet with the icon
embedded), `upstart/` and `ui/`. `manifest.json` takes its version from
`Cargo.toml` and declares only `kindlehf` (ADR 0001).

**`install.sh`** must be safe to run again over a partial or older install,
because rerunning it is how a failed install gets fixed:
1. Copy the binary and scripts to `/var/local/kindle-wireless-sync/` as
   `*.new` and then `mv` them into place. This works while the service is
   running, and a running `sh` script isn't changed under it. Replace `ui/`
   as a whole, and clear the mesquite cache.
2. Register the app, and install the scriptlet the same way (`*.new` then
   `mv`).
3. Install each upstart job only if `cmp` shows it changed, with
   `mntroot rw`/`ro` and a trap. Then run `initctl reload-configuration`.
4. Clear `boot_attempts`. Leave the autostart sentinel alone: a fresh
   install has autostart **off** (ADR 0002), and an upgrade keeps the user's
   setting.
5. Restart the service if it was running. Start it on a fresh install. On an
   upgrade where the user had stopped it, leave it stopped. A fresh service
   with no allowed library contacts nobody (ADR 0003), so starting it is
   safe, and the app works straight away.

**`uninstall.sh upgrade`** exits 0 immediately, so settings, the allowlist
and the service survive. `install.sh` does the replacing.

**`uninstall.sh`** (removal) stops the app (`appmgrd stop`) and the service,
removes the job files (`mntroot`, with a trap) and reloads upstart. It
unregisters the app using the package's own copy of `register-app.sh`, then
removes the scriptlet and its `.sdr/`, `/var/local/kindle-wireless-sync/`
(binary, settings, allowlist, passwords, `books.json`, cover backups) and
`/var/log/kindle-wireless-sync.log`. **It keeps the received books and
their thumbnails,** because they are the user's. Failures stop the script
with a non-zero exit, so KPM keeps the package and the user can retry.

**`launch.sh`** runs the packaged scriptlet, so `kpm launch` opens the app
just as the Library tile does.

## Alternatives considered
- **Running from the KPM package directory** (`/mnt/us/kmc/...`) instead of
  copying to `/var/local`: rejected by ADR 0004. That directory is on the
  USB-visible FUSE storage, and KPM deletes it and extracts it again on
  every upgrade, while the service may be running from it.
- **Stopping the service for the whole install:** not needed, since
  copy-then-`mv` is safe while it runs, and the restart at the end loads the
  new binary. It would also mean deciding what "was running" means after a
  failure.
- **Keeping settings after removal** (as HID Passthrough keeps
  `devices.conf`): rejected for now. Our settings are small, and the only
  sensitive part is passwords. The user expects removal to leave nothing
  behind except their books. Calibre resends book metadata after a
  reinstall.
- **Rolling back inside `install.sh` on failure:** rejected as too complex
  for the benefit. Each step can be repeated, so installing again recovers.

## Consequences
- An install that fails partway may leave our files with no KPM record. The
  fix is to install again, then uninstall if the user wants it gone.
  `uninstall.sh` only removes things that exist, so it copes with partial
  installs.
- Local `file://` installs skip KPM's platform check, so a user on another
  platform can still install. ADR 0001 makes other devices best-effort.
- Testing: a recipe installs the built package with the device's own
  `kpm install file://...` and removes it with `kpm uninstall`.
