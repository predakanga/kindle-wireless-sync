# 0010: Autostart sentinel and crash-loop guard

- Status: Accepted
- Date: 2026-10-15

## Context
ADR 0004 installs two upstart jobs once and controls autostart without
touching the read-only root filesystem. This ADR records exactly how the
autostart and its safety net work, and where the ideas came from.

Prior art:
- **kindle-usbnetlite** (github.com/notmarek/kindle-usbnetlite,
  `extension/usbnetlite.conf`): an always-installed upstart job whose
  `pre-start` only does its work if a **sentinel file** exists
  (`/mnt/us/usbnetlite/auto`). Our autostart sentinel follows this idea. We
  moved the check into a separate task so manual starts aren't blocked (ADR
  0004).
- **Kindle Button Mapper** and **Kindle HID Passthrough**
  (github.com/zampierilucas/kindle-button-mapper-rs,
  github.com/zampierilucas/kindle-hid-passthrough, their
  `assets/*.upstart`): a **boot-loop guard**. `pre-start` keeps a
  `boot_attempts` counter and turns autostart off after 3 starts without a
  stable run. Our crash-loop guard follows this.

## Decision
Files in `/var/local/kindle-wireless-sync/` (persistent, writable, not
exposed over USB):

| File | Meaning |
|---|---|
| `autostart` | Sentinel. If present, start the service at boot. The UI toggle creates or deletes it. |
| `boot_attempts` | Number of boots since the service last ran stably. |
| `state/boot.log` | Note written when the guard turns autostart off. |
| `autostart_disabled` | Marker the guard creates when it turns autostart off. The UI shows it, and turning autostart on removes it (ADR 0011). |

Flow:
1. `kindle-wireless-sync-autostart.conf` (`start on framework_ready`,
   `task`) runs `autostart.sh` once per boot.
2. `autostart.sh` does nothing unless `autostart` and the binary both exist.
   If `boot_attempts` ≥ 3, it **deletes `autostart` and the counter**,
   writes to `boot.log`, and exits. Otherwise it increments the counter and
   runs `start -n kindle-wireless-sync`.
3. `service.sh` (run by `kindle-wireless-sync.conf`) starts a background
   check that **deletes `boot_attempts` if the daemon is still running 120 s
   later**, then `exec`s the daemon.
4. Upstart's `respawn limit 5 60` stops a daemon that crashes repeatedly
   within one boot. The counter covers crashes that span boots, for example
   a crash that takes the device down with it.

Manual starts (`initctl start`, the UI) never touch the counter, and the
sentinel never blocks them.

## Alternatives considered
- Counter in the main job's `pre-start` (the references' placement): it
  would also count manual starts and respawns, so a few manual restarts
  could turn autostart off.
- No guard: the daemon is not a boot dependency (started with `-n`, and
  nothing waits on it), so it can't hang boot. But a crash loop could still
  cost battery or destabilise the device on every boot. The guard is cheap.

## Consequences
- Verified on the reference device (2026-10-15): with `autostart` present,
  the service started about 65 s after a reboot, `boot_attempts` read 1,
  the daemon connected to Calibre within seconds, and `boot_attempts` was
  removed once it had run for 2 minutes.
- Not yet exercised: the guard actually tripping (3 bad boots).
- The UI shows "autostart was turned off after repeated failures" while the
  `autostart_disabled` marker exists.
