# 0012: KPM package install, upgrade and uninstall

- Status: Accepted
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

## Findings from the device test (2026-10-15)
All on the PW12 with KPM 0.2.2:
- **The package works.** Install over an existing (development) setup,
  upgrade, uninstall, and a fresh install all behaved as described above.
  After the fresh install, Find libraries, Allow and a book sent from
  Calibre worked end to end. The root filesystem was remounted only when
  job files were added or removed, and was read-only afterwards.
- **KPM bug: a local file can't replace an installed package.**
  `kpm install file:///x.kpkg` fails with "Could not resolve dependency
  graph" when the package is already installed, whatever the version. KPM
  leaves an installed package out of the graph only if its id equals a
  target's id (`src/install.c`, the `installing` check). For a `file://`
  target the "id" is the URL, so the installed copy stays in the graph and
  conflicts. Master (`215736c`) has the same code. So a user who installed
  from a file must uninstall (losing settings) before installing a newer
  file. Upgrades from a **repository** work: tested by serving this package
  from a temporary `file://` `repo.json` (`kpm add-repo`, `update`,
  `install`). KPM ran `uninstall.sh upgrade`, then `install.sh`, and the
  settings, allowlist and autostart were kept.
- After uninstall, the scanner removed the tile's catalog entry 10–15 s
  later, slower than the ~2 s seen for books, probably because KPM starts a
  full rescan.

**Coming in KPM after 0.2.2 (master, unreleased):** `install.sh` gets an
`upgrade` argument (`7471a4e`), which ours ignores. If `install.sh` fails,
KPM runs `uninstall.sh` *without* `upgrade` (`67233d9`), so a failed upgrade
removes the app and its settings, but not the books.

## Hooks inherit the caller's umask (2026-10-17)
KPM runs `install.sh` with the umask of the shell that ran `kpm`. On the
emulated Kindle that shell came from the emulator's own dropbear, which
runs with umask `0077`, so `mkdir -p` and `cp -r` left the app directory
and its files `0700`, owned by root. The firmware's mesquite runs apps as
the framework user (uid 9000), so it couldn't read them. The app failed
with "The selected application could not be started"; the log shows
`JunoApplicationManager … error=Unsupported content path for application
content`. appreg.db was correct. The firmware's own default is gentler
(`/etc/profile` sets `umask 002`, appmgrd runs with `0002`), so whether a
real device sees this depends on how the user reached a shell.

`install.sh` now sets `umask 022` and the modes it needs explicitly:
`755` for `/var/local/kindle-wireless-sync` and `ui/`, `644` for the app's
files, and `700` for `state/`, which holds library passwords. The `chmod`s
also repair an earlier install on upgrade. Tested on the emulator: with the
0.2.0 install's `0700` modes, running the new `install.sh` over the
umask-`0077` SSH (as KPM runs its hooks) fixed the modes, and the app
opened.

## Upgrades close the app (2026-10-17)
The firmware keeps a backgrounded app's mesquite process, with its page and
script in memory. On the device, an app opened at 00:45 was still running
after a KPM upgrade at 21:37, so reopening it resumed the 0.2.0 script
against the new service (it showed the raw state `not_linked` and no found
libraries). Clearing mesquite's cache (`/var/local/mesquite/<app id>`)
doesn't help with that. `install.sh` now closes the app before replacing
its files, with `lipc-set-prop com.lab126.appmgrd stop app://<app id>`,
the same graceful unload `uninstall.sh` uses (killing mesquite makes
appmgrd report a crash). Tested on the device: with the app backgrounded
behind Home, a repository upgrade closed it and Home stayed in front.

## `initctl reload-configuration` failing (2026-10-18)
Twice on the emulated Kindle (2026-10-17 and 2026-10-18), a KPM install
stopped at `initctl reload-configuration`: "initctl: Did not receive a
reply", so KPM recorded the install as failed. Installing again worked.

Measured with the same three rounds on both machines: 50 reloads in a row
with the service running; 50 with the service stopped or started between
reloads; and 12 alternating with creating, editing and deleting a test job
in `/etc/upstart` (each change its own `mntroot rw` and `ro`):
- **The Kindle** (up 1.5 days): 112 reloads, none failed, the slowest
  0.17 s.
- **The emulated Kindle**, in its first minutes after boot (load average
  about 3): 1 failure in those 112, and 1 in 100 more. Each failed within
  half a second, so upstart's reply was lost rather than late.
- **The emulated Kindle once settled** (up 8 minutes, load 0.1): 200
  reloads, none failed.

So it's the emulator while busy after boot, which is when both failed
installs ran. `install.sh` and `uninstall.sh` now try once more after 2 s
before giving up, which covers that case, and costs nothing when the first
attempt works.
