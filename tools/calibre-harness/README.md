# Calibre test harness

A development-only Calibre plugin, plus a client that launches Calibre with
it and sends it commands (ADR 0019). Calibre runs isolated: its config,
caches and library are in `dev-calibre/`, and your own Calibre never gets
the plugin. Quit your own Calibre first, since only one Calibre GUI can run
at a time.

```sh
just calibre launch                  # start Calibre with dev-calibre/library
just calibre wireless-start          # Connect/share → Start wireless device connection
just connect --allow <library-uuid>  # in another terminal: the host-side device
just calibre wait-device
just calibre send 1 --convert yes --wait
just calibre device-books
just calibre field '#kindle_read'
just calibre delete "Title - Author (1).txt" --wait
just calibre dialogs                 # any error dialog shows up here
just calibre quit
```

`harness.py --help` and its docstring list every command. The library's
UUID is in `just calibre status`. To prepare the library, run `calibredb
--library-path dev-calibre/library add ...` (with
`CALIBRE_CONFIG_DIRECTORY=dev-calibre/config`) while the harness Calibre
isn't running.

Things to know:
- **Questions are modal.** Code that might ask one goes through `exec
  --no-wait`. Answer it with `dialogs --accept` or `--dismiss`. `send
  --convert yes|no` answers the auto-convert question in advance.
- **A conversion adds the converted format to the library book**, so
  sending the same book again doesn't ask.
- **`quit` can hang.** Once (2026-10-17), after the device's service was
  stopped, Calibre hung in `stop_plugin` → `eject` (the end of
  `dev-calibre/calibre.log`), and SIGTERM and `osascript` didn't end it.
  Only `kill -KILL` did. The library was fine afterwards.
- **Dropping the device mid-job:** freeze the device with `kill -STOP`,
  start a send, wait for `status` to show the upload job running, then
  `kill -9` it (ADR 0019, implementation notes).

## `exec` snippets in use

Named commands should cover what tests do repeatedly. Until a snippet
earns one, record it here whenever you use it, with the date. One that
shows up in several sessions should become a command.

- Remove a format from a library book (to make a send ask about
  converting again). Used 2026-10-17.
  `db.remove_formats({2: ('AZW3',)})`
- Update the device's cached metadata (*Update cached metadata on device*).
  Used 2026-10-17.
  `gui.update_metadata_on_device(); result = "ok"`
- Switch Calibre to another library (with `--no-wait`), here a second one
  made with `calibredb --library-path dev-calibre/library2 list`. Used
  2026-10-18.
  `gui.library_moved('/abs/path/dev-calibre/library2'); result = "ok"`
