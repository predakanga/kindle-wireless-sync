# 0004: UI, daemon and service layout

- Status: Accepted
- Date: 2026-10-15
- Amended: 2026-10-15. Service and directory names changed from `kws` to
  `kindle-wireless-sync` (naming decision in ADR 0006).
- The autostart and crash-loop guard mechanics are detailed in ADR 0010.
- Amended by [0020](0020-one-screen-pages-and-checking-a-library.md)
  (2026-10-18): the app has Restart and Stop, not Start and Stop.

## Context
We need an on-device UI for the allowlist, service start/stop, autostart and
status (ADR 0002, 0003). Both reference projects (kindle-button-mapper-rs and
kindle-hid-passthrough) use the same pattern on current firmware: an
HTML/CSS/JS **WAF app** run by the system `mesquite` engine, registered in
`appreg.db` and opened from a scriptlet in `documents/`. The app talks to the
process over a **localhost HTTP JSON API**. See `docs/research/kindle-platform.md`.

For autostart, both references install or remove their upstart `.conf` file.
That needs `mntroot rw` on every toggle, because `/` is a read-only ext3
filesystem. kindle-usbnetlite instead always installs its job and checks a
sentinel file (`/mnt/us/usbnetlite/auto`) in `pre-start`. That approach has
problems of its own:
- The sentinel also blocks manual starts.
- The job file can't change without going rw. usbnetlite even remounts rw at
  boot to repair files.
- The job runs files from user storage, which is reachable over USB.

Device facts (FW 5.20.1): upstart 0.6.6, and `start` supports `-n/--no-wait`.
`/var/local` is persistent rw ext4 with ~397 MB free and isn't exposed over
USB. `/mnt/us` is a FUSE layer (`fsp`) over ext4 and is what the user sees
over USB.

## Decision

**Process and UI**
- One static Rust binary (`armv7-unknown-linux-musleabihf`) runs the sync
  daemon and serves a JSON API on `127.0.0.1`.
- The UI is a WAF app made of static HTML/CSS/JS, launched from a
  `documents/` scriptlet and registered in `appreg.db` at install time.

**Service layout: two job files, installed once**
- `/etc/upstart/kindle-wireless-sync.conf` is the daemon. It has **no `start on`**, so it runs
  only when started explicitly. It has `stop on stopping filesystems`,
  `respawn` with a limit, and the boot-loop guard. `pre-start` exits cleanly
  if the binary is missing.
- `/etc/upstart/kindle-wireless-sync-autostart.conf` is a `task` with `start on
  framework_ready`. If the autostart sentinel and the binary exist, it runs
  `start -n kindle-wireless-sync`. Otherwise it does nothing, and it never blocks boot.
- Both files stay minimal and stable. They only `exec` the binary or a script
  stored on writable storage, so all logic can be upgraded without touching
  `/etc`.

**Controls (no rootfs writes)**
- Start/stop from the UI runs `initctl start|stop kindle-wireless-sync`. The sentinel never
  gets in the way of a manual start.
- The autostart toggle creates or deletes the sentinel.
- The boot-loop guard turns autostart off by deleting the sentinel.
- Network-up isn't an upstart event. The running daemon waits for
  `com.lab126.wifid cmStateChange "CONNECTED"` before discovering or
  connecting, and pauses when the network goes down (see
  `kindle-library-integration.md`, network-up events). So "autostart at boot"
  and "autostart on network-up" from ADR 0002 are one mechanism: start at
  boot, connect when the network comes up.

**Locations**
- Binary, sentinel, config/allowlist and state go in **`/var/local/kindle-wireless-sync/`**:
  persistent, writable, and not exposed over USB.
- `install.sh` copies the binary there from the KPM package directory.
- WAF app files live wherever the `appreg.db` `command` points.

**Root filesystem writes** happen only in `install.sh` (to add the two job
files, and only if their content changed) and in `uninstall.sh` (to remove
them). Each is a `mntroot rw` → change → `mntroot ro` sequence with a trap.

## Alternatives considered
- Installing or removing the job `.conf` on every toggle (both references):
  rejected, because a UI action would need a rootfs remount.
- A single job that checks a sentinel (usbnetlite): rejected, because it
  blocks manual starts or would need fragile upstart environment tricks.
- Binary and state on `/mnt/us` (both references): rejected. Root would run
  files that anyone with USB access can replace. The cost is that during
  development the binary can't be swapped over USB, but the justfile deploys
  over SSH anyway.
- A native framebuffer UI (FBInk etc.): rejected. It's more work, it looks out
  of place in the Kindle UI, and the references didn't take that route.
- A separate helper process that runs only while the UI is open (button
  mapper's approach): rejected. The daemon already has to run for sync, so
  embedding the API is simpler.
- A KUAL extension (a menu entry in the KUAL launcher, with no `appreg.db`
  registration): rejected. KUAL doesn't currently work on `kindlehf`, which is
  our reference platform (the developer reported this on 2026-10-15).

## Consequences
- The UI can only reach the API while `kindle-wireless-sync` is running. When it isn't, the
  scriptlet that opens the UI runs `start -n kindle-wireless-sync` first. The UI's "stop"
  pauses syncing inside the daemon rather than killing the API it depends on.
  The exact split between the "process running" and "sync enabled" states is
  settled when the UI is built.
- An OTA update can wipe `/etc/upstart`. The UI must detect a missing job
  ("service not installed, reinstall") instead of failing silently.
- If our files disappear without uninstall running, the job files are left
  behind. They must exit cleanly when the binary is missing.
- We need a small HTTP server in the binary. Pick a minimal crate to keep the
  binary small.
- Installing and uninstalling must manage `appreg.db` rows and the mesquite
  cache. `appreg.db` (`/var/local/appreg.db`) is the system's handler
  registry. It maps file globs and MIME types to extractors and apps, and it
  holds each handler's properties. `appmgrd` looks it up to resolve
  `app://<id>`. We add one `handlerIds` row, one `associations` row
  (`application`, `GL:<id>`) and the `command`/`lipcId`/`supportedOrientation`
  properties under our own handler ID, and remove exactly those on uninstall.
  The reference projects already do this on the reference device.
