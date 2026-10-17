# 0019: Drive Calibre through a development-only plugin

- Status: Proposed
- Date: 2026-10-17

## Context
`tests/replay.rs` covers the wireless protocol without Calibre. Everything
past the protocol has only been tested by hand in Calibre's GUI. That
includes the GUI's book matching on connect, metadata updates, applying
read-sync values to library columns (ADR 0017), and the error dialog
Calibre shows when the device drops mid-job. ADR 0017's device test found
three bugs this way, each after several rounds of the developer clicking
through Calibre and reading its log at exit. We want to script those
checks.

That logic runs in Calibre's GUI process, not in the driver. On connect,
`DeviceMixin.device_detected` (`src/calibre/gui2/device.py`) queues
`get_device_information`, `set_library_information` and `books`.
`metadata_downloaded` then calls `set_books_in_library`, which runs the
driver's `synchronize_with_db` and swallows its exceptions. Device job
failures go to `device_job_exception`, which shows the shared
`gui.device_error_dialog`. Only code inside the GUI process can trigger
these steps or see their results.

Calibre 9.16 facts that shape the design (source at tag `v9.16.0`):
- The wireless server is a "dynamic plugin". The GUI starts and stops it
  with `device_manager.start_plugin('smartdevice')` and `stop_plugin`. Its
  settings go through `set_option` (`password`, `use_fixed_port`,
  `port_number`, `autostart`). The menu item opens `SmartdeviceDialog`,
  which does the same thing (`gui2/actions/device.py`,
  `gui2/dialogs/smartdevice.py`).
- Only one GUI runs per user. The single-instance lock and the GUI socket
  (`/tmp/calibre-<uid>-gui.sock` on macOS, `utils/ipc/__init__.py`) don't
  depend on the config directory.
- `CALIBRE_CONFIG_DIRECTORY` and `CALIBRE_CACHE_DIRECTORY` move the config
  (plugins and preferences included) and the caches (including the
  wireless metadata cache) (`constants.py`).
- A fresh config shows the welcome wizard unless the `dynamic` preference
  `welcome_wizard_was_run` is set (`gui2/main.py`, `run_gui_`).
- `calibre-debug -g` runs the GUI with its debug output on stdout. Extra
  arguments after `--` go to `calibre`.

## Decision
A Calibre **GUI plugin** that only development uses, in
`tools/calibre-harness/plugin/`, and a client script,
`tools/calibre-harness/harness.py`, that launches Calibre with it and sends
it commands.

- **An isolated Calibre.** The client launches Calibre with its config,
  caches and library under `dev-calibre/` (gitignored, like `dev-device/`).
  It sets the welcome-wizard flag and installs the plugin with
  `calibre-customize -b` into that config, so the developer's own Calibre
  never has the plugin. It **refuses to launch** while another Calibre is
  running, and never quits one it didn't start.
- **The plugin** is an `InterfaceAction` with no toolbar button. In
  `genesis()` it starts an HTTP server on `127.0.0.1` with a random port and
  token. It writes both to `kws-harness.json` (mode 0600) in the config
  directory. It does nothing unless the launcher has set `KWS_HARNESS=1`, so
  installing it by mistake into a normal Calibre is harmless.
- **Each request runs on the GUI thread**, passed over with a queued Qt
  signal. Calls block the server thread, not the GUI. Commands never wait
  inside the GUI: the client waits by polling `status`.
- **Commands:** `status` (library, wireless server, device, jobs, open
  dialogs), `wireless-start` (optional port and password) and
  `wireless-stop`, `device-books` (Calibre's view of the device list),
  `dialogs` (optionally dismissing them), `quit`, and `exec`. The client
  adds `launch` and `wait-device`. `wait-device` waits until a device is
  connected and no jobs are left. `dialogs` lists every visible dialog with
  its text and details. It is how a test asserts that Calibre showed no
  error.
- **`exec`** runs Python on the GUI thread with `gui`, `db` (the current
  library's `new_api`) and `dm` (the device manager) in scope, and returns
  `result`. It's for exploring. A snippet that tests come to rely on
  becomes a named command.
- The plugin is **not a user requirement and not shipped**: it is not in
  the `.kpkg`, the release or the README's install steps.

## Alternatives considered
- **Simulating clicks** (AppleScript or computer use): Qt exposes little to
  macOS accessibility, the wireless item is in nested menus, and timing and
  focus make it flaky. It also can't read results such as what Calibre
  thinks is on the device or a column's value. Still fine for a one-off
  look at a dialog.
- **Driving the driver headless** (`calibre-debug -e` with
  `SMART_DEVICE_APP`): this runs Calibre's real server code without the
  GUI, but leaves out the GUI logic above, where the bugs have been. The
  replay tests already cover the protocol.
- **`calibredb` alone:** good for preparing a library, but it can't trigger
  device actions. It also mustn't write to a library the GUI has open.
- **A fixed set of commands with no `exec`:** safer, but every new check
  would need a plugin change and a Calibre restart before we know what's
  worth checking. The token and loopback binding cover the risk for a tool
  that only runs during development.

## Consequences
- Calibre-side checks can be scripted. With `just connect` acting as the
  device, a full send/sync/check loop can run on the Mac without the
  Kindle.
- The test library has its own UUID. The Kindle (or `just connect`) has to
  be allowed or linked to it (ADR 0018), separately from the developer's
  real library.
- The developer has to quit their own Calibre while the harness runs.
- The plugin relies on GUI internals (`device_manager`,
  `device_error_dialog`, `booklists()`), which can change between Calibre
  versions. It's checked against 9.16.
- It's development-only Python in a Rust project. It isn't covered by
  `just check` or CI.
- A Calibre plugin for users (Later item 5) is a separate decision. This
  one doesn't count as that plugin.
