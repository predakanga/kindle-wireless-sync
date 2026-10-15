# 0015: Report the Kindle's own name to Calibre

- Status: Accepted
- Date: 2026-10-16

## Context
Phase 5's low-priority stretch goal: use the Kindle's own device name as
the device name Calibre shows. Until now we always sent "Kindle", or
whatever `--name` set.

Evidence:
- **On the device** (reference PW12, FW 5.20.1):
  `com.lab126.amazonRegistrationService` has a readable `GetDeviceName`
  property, which holds the name from *Settings → Device Options → Device
  Name*. It also has a writable `SetDeviceName`, which we never use.
  `lipc-probe` lists no change event. `com.lab126.dpmManager
  isEditDeviceNameDisabled` is 0. Other properties of the same service hold
  account details, so we read only `GetDeviceName`.
- **Calibre** (`src/calibre/devices/smart_device_app/driver.py`): the
  handshake reply's `deviceName` becomes `client_device_name`, and
  `get_gui_name()` shows it as "Wireless device: <name>". That's the name in
  the status bar ("Device: … detected"), the busy message to other devices,
  and the device configuration dialog's title. The driveinfo's
  `device_name` is a separate field. Calibre fills it once, when it is
  missing (`_update_driveinfo_record`), and changes it only when the user
  renames the device in Calibre (`set_driveinfo_name`, which arrives as
  `SET_CALIBRE_DEVICE_NAME`). Calibre's location bar doesn't show it
  (`gui2/layout.py`, `update_devices`).

## Decision
- **Before each connection**, the daemon reads `GetDeviceName` and sends it
  as `deviceName`. A rename on the Kindle shows from the next session, and
  sessions restart often, since Wi-Fi drops with the screensaver. We don't
  watch for changes.
- **Cleaning:** trim, drop control characters, and cap at 64 characters. If
  nothing is left, or `lipc` isn't available (as off a Kindle), we send
  "Kindle". `--name` still overrides.
- **The driveinfo `device_name` is Calibre's**, and we leave it alone. We
  only supply a value when it is missing, as before: on first contact it now
  starts as the Kindle's name instead of "Kindle".
- The Find libraries and Add server probes disconnect before replying to
  the handshake, so they send no name and are unchanged.

## Alternatives considered
- **Watching for changes and reconnecting:** there's no event, and polling
  costs more than it's worth for a cosmetic change that already shows at the
  next session.
- **Also rewriting the driveinfo `device_name`:** that would overwrite a
  name the user chose in Calibre, and Calibre's GUI doesn't show this field
  anyway.

## Consequences
- The Kindle's name goes to the user's own Calibre on the LAN, as the
  handshake already did with "Kindle". It isn't logged or stored.
- One `lipc-get-prop` call per connection attempt. That's cheap, and
  attempts are at least 10 s apart (ADR 0008).
