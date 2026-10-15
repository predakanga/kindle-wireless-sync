# 0004: UI as a WAF app over a localhost API

- Status: Proposed
- Date: 2026-10-15

## Context
We need an on-device UI for the allowlist, service start/stop, autostart and
status (ADR 0002, 0003). Both reference projects (kindle-button-mapper-rs and
kindle-hid-passthrough) use the same pattern on current firmware: an
HTML/CSS/JS **WAF app** run by the system `mesquite` engine, registered in
`appreg.db` and opened from a scriptlet in `documents/`. The app talks to the
process over a **localhost HTTP JSON API**. See `docs/research/kindle-platform.md`.

## Decision
- Ship one static Rust binary (`armv7-unknown-linux-musleabihf`). It runs the
  sync daemon under upstart and serves a JSON API bound to `127.0.0.1`.
- The UI is a WAF app built from static HTML/CSS/JS, launched from a
  `documents/` scriptlet and registered in `appreg.db` at install time.
- Autostart is on exactly when the upstart `.conf` file exists, following HID
  passthrough. Start/stop is a flag inside the daemon, so the UI can always
  reach the API.

## Alternatives considered
- Native framebuffer UI (FBInk etc.): rejected. It's more work, it looks out
  of place in the Kindle UI, and the references didn't take that route.
- A separate helper process that runs only while the UI is open (button
  mapper's approach): rejected. The daemon already has to run for sync, so
  embedding the API is simpler.
- A KUAL extension (a menu entry in the KUAL launcher, with no `appreg.db`
  registration): rejected. KUAL doesn't currently work on `kindlehf`, which is
  our reference platform (the developer reported this on 2026-10-15).

## Consequences
- The UI can only reach the daemon while the daemon process is running. If
  autostart is off, opening the app has to start the job first, from the
  scriptlet.
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
