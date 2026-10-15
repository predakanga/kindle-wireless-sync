# Kindle platform: packaging, UI, services

Research for Phase 1, item 6. Sources:

- kindle-button-mapper-rs (master, cloned 2026-10-15): `justfile`,
  `.cargo/config.toml`, `kpm/`, `install.sh`, `uninstall.sh`, `assets/*.upstart`,
  `illusion/`, `.github/workflows/build-arm.yml`.
- kindle-hid-passthrough (master, cloned 2026-10-15): `assets/hid-passthrough.upstart`,
  `kpm/`, `kindle_hid_passthrough/api_server.py`, `power_monitor.py`, `CLAUDE.md`.

## Build target

- `armv7-unknown-linux-musleabihf`, statically linked, using
  `linker = "rust-lld"` with `-C link-self-contained=yes`
  (`.cargo/config.toml`). No cross toolchain or glibc needed.
- Release profile: `lto = true`, `strip = true`, `opt-level = "z"`,
  `panic = "abort"`, `codegen-units = 1`.
- KPM platforms both projects declare: `kindlehf` and `kindlepw2`. **Still to
  check:** which one our Paperwhite 12 on 5.20.1 reports. A hard-float binary
  is expected to cover it.

## KPM package

- A `.kpkg` is a **gzip tarball** with `manifest.json` **at the archive root**
  (no `./` prefix; KPM fails to find it otherwise).
- `manifest.json` (manifest_version 2): `id`, `name`, `author`,
  `description`, `version` (int array), `dependencies`,
  `supported_platforms`.
- KPM runs `install.sh` from the package. It runs `uninstall.sh` on removal
  and also on upgrade with argument `upgrade`. Both projects exit 0 straight
  away on `upgrade`, so user state survives.
- Distribution uses a `repo.json` (manifest_version 1) listing packages and
  artifact URLs. Both projects generate `manifest.json` from `Cargo.toml` or
  `repo.json` with `jq`, and CI checks that `repo.json` is not stale.

## Installing system files

- The root filesystem is read-only. Use `/usr/sbin/mntroot rw` and then
  `mntroot ro`, with a trap so it is always set back to ro.
- App state lives under `/mnt/us/<app>/` (the user-visible FAT partition,
  which is case-insensitive). Logs go to `/var/log/<app>.log`.
- On FAT, a running binary can't be overwritten. Copy to `*.new`, then `mv`.

## Service (upstart)

- The upstart job goes in `/etc/upstart/<name>.conf`. After adding or removing
  it, run `initctl reload-configuration`, then use
  `initctl start|stop|restart|status <name>`.
- Start conditions in use: `start on framework_ready` (HID passthrough) and
  `start on mounted_userstore or user_store_available` (button mapper).
  Stop condition: `stop on stopping filesystems`.
- `respawn` plus `respawn limit 5 60`, and a `pre-start` **boot-loop guard**:
  a counter in `boot_attempts`. After 3 starts without a stable run, autostart
  turns itself off. The daemon resets the counter once it is running stably.
  We should copy this, because a crash-looping job at boot can make the
  device unusable.
- **Autostart toggle (HID passthrough):** autostart is on exactly when the
  upstart `.conf` file exists. The UI turns it on or off by installing or
  removing that file. Start/stop of the *feature* is separate: a flag inside
  the running daemon.
- **Network-up trigger:** neither project does this. Find the relevant lipc
  events on the device (`com.lab126.wifid` / `com.lab126.cmd` connection
  state) as a Phase 3 task.

## Power

- HID passthrough follows `lipc-wait-event -m -s 0 com.lab126.powerd` for
  `goingToScreenSaver` and `readyToSuspend`, and drops connections on suspend.
  We'll need the same, because Wi-Fi goes down when the device sleeps.
- During development: `lipc-set-prop -i com.lab126.powerd preventScreenSaver 1`.

## On-device UI: WAF ("illusion") app

Both projects build their UI the same way. It is not a native GUI toolkit:

- An HTML/CSS/JS **WAF app** run by the system `mesquite` browser engine. A
  W3C widget `config.xml` declares the app id, the Kindle API features
  (`appmgr`, `messaging`, `chrome`, ...) and the gestures.
- It's registered in `/var/local/appreg.db` (sqlite) as a handler with
  `command = /usr/bin/mesquite -l <app-id> -c file://<app-dir>/`, and launched
  with `lipc-set-prop com.lab126.appmgrd start app://<app-id>`.
- A shell **scriptlet** in `/mnt/us/documents/<Name>.sh`, with `# Name:` and
  `# Icon:` header comments, makes the app show up as an item in the Kindle
  library. Tapping it registers or launches the app.
- The app talks to a **localhost HTTP JSON API** served by the Rust/Python
  process: port 8322 for the button mapper (a helper started only while the
  app is open), and port 8321 for HID passthrough (embedded in the daemon).
- Uninstalling removes the appreg rows, `/var/local/mesquite/<app-id>` and the
  scriptlet.

## Dev workflow (from the justfile)

- An SSH host alias `kindle`. Recipes: `build-kindle`, `deploy` (stop
  service, mntroot rw, scp binary as `.new` then mv, install upstart conf),
  `start/stop/restart/status`, `logs` (`tail -f`), `deploy-watch`, and
  `deploy-waf`, which restarts the helper, clears the mesquite cache and
  relaunches the app.
