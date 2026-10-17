# 0020: One screen per page, and checking a library before linking it

- Status: Accepted (developer, 2026-10-18)
- Date: 2026-10-18
- Amends [0004](0004-ui-architecture.md) (service controls),
  [0011](0011-localhost-api.md) (the API), [0017](0017-read-status-sync.md)
  (the user chooses the columns, which move into `config.json`) and
  [0018](0018-one-linked-library.md) (linking checks the library first).

## Context
**Scrolling.** The developer's UI review (2026-10-17) started from one
complaint: the app is about one and a half screens tall, and mesquite
scrolls a few pixels per swipe, so reaching the bottom takes about ten
swipes. The developer asked for pages that each fit one screen.

**Linking.** The developer also asked for a different linking flow: choose a
library, let the service check it (is it there, is the password right, what
columns does it have), choose the settings, and only then link and connect.
Today linking only writes `config.json` (ADR 0018). Read sync uses two fixed
column names (ADR 0017), though the daemon already reads whatever names
`config.json` holds.

**Prototype.** `tools/ui-prototype` (`just ui-prototype`) is a clickable
version of the app with a fake service and fake Calibre libraries. The
developer reviewed it in three rounds of notes (2026-10-17 and 2026-10-18).
The pages below are what it settled on. It measures each page against the
Paperwhite 12th gen's screen: 1272×1696, of which the app gets about
1272×1483 below the Kindle's bars (measured from a screenshot). Every page
fits, most with half the screen spare.

**The Kindle's browser ignores flexbox.** On the device, the current app's
`display: flex` rows don't stretch: its Link and Password buttons sit
against the library name, and Connect now and Pause keep their natural
width. kindle-hid-passthrough's BTManager app avoids flexbox for the same
reason and lays out with floats.

**What Calibre allows a check to do** (Calibre 9.16.0 source):
- **Columns arrive late.** Calibre sends the library's columns in
  `SET_LIBRARY_INFO`, after the handshake in which the device names its
  read-sync columns (`research/calibre-wireless-protocol.md`, connect
  sequence). So only a connection can learn them.
- **The connection is one job.** `gui2/device.py` runs device information,
  `set_library_information` and the book list as steps of one job
  (`device_detected`, `info_read`). If the device leaves during that job,
  `device_job_exception` shows an error dialog. A check therefore has to
  answer the book list, then leave at a keep-alive once Calibre is quiet,
  as read sync's reconnect does (ADR 0017, implementation notes). Calibre
  ignores a failed keep-alive (`detect_managed_devices`).
- **The password is checked first.** `driver.py` `open()` sends a
  `passwordChallenge` only when Calibre has a password, and checks the reply
  in the handshake. A wrong one gets a `DISPLAY_MESSAGE` and a quiet close
  (`OpenFailed('')`, "Don't bother with a message").
- **The cheap part already exists.** `daemon::probe`, used by Find
  libraries, reads Calibre's first message (library name, UUID and whether a
  password challenge is set), then disconnects.
- **Calibre locks out an address after 5 failed connections.**
  - **What counts.** `detect_managed_devices` counts every connection
    from an address (`connection_attempts[peer]`), and `open()` resets the
    count when a handshake succeeds. So each Find libraries probe and each
    wrong password counts.
  - **What happens at 5.** Calibre closes every new connection from that
    address before sending anything (`MAX_UNSUCCESSFUL_CONNECTS`). One
    can't succeed to reset the count.
  - **What clears it.** Only stopping and starting Calibre's wireless
    device connection, or restarting Calibre (`_startup_on_demand` empties
    the counts).
  - **What doesn't.** The count is keyed on the address, so no device ID or
    restart on the Kindle's side helps.
- **A check can't use the Kindle's own device ID.** Calibre keeps a
  metadata cache per device ID (`device_store_uuid`):
  - With `cacheUsesLpaths`, `books()` deletes cache entries for every book
    the device doesn't list. Reporting no books under the real ID would
    empty Calibre's cache for this Kindle, and the next real session would
    fetch every book's metadata again.
  - Reporting the real books to a library that isn't linked yet is the
    stale-cache problem ADR 0018 solved by giving each library its own
    device ID.
- **A device ID costs a file.** `get_device_information` makes up an ID for
  a device that has none. When the connection closes, `_close_device_socket`
  writes `wireless_device_<id>_metadata_cache.json`, even when it's empty.
- **New columns need a Calibre restart.** `gui2/preferences/columns.py`
  marks every new or deleted column as needing one. A restart also stops
  Calibre's wireless connection, unless "Automatically start" is ticked in
  its wireless dialog (`gui2/dialogs/smartdevice.py`, `autostart`).

## Decision

### Pages
The app becomes a set of pages that each fit one screen, with no scrolling.
The app opens on the page for the current state. The prototype
(`tools/ui-prototype/app/`) is the reference for layout and wording.

- **Choose a Calibre library**: the first page while nothing is linked.
  It searches when it opens, and lists what it finds; each row starts a
  check. **Search again** and **Connect by address…**. While Wi-Fi is off,
  a warning takes the place of the list and Search again is greyed out.
  Reached from Home's **Change library**, it has a back arrow instead.
- **Connect by address**: computer name or address, port and password at
  the top of the page (the on-screen keyboard covers the bottom), and
  **Connect**, which starts a check.
- **Testing connection to *library*…**, with "Calibre will show the Kindle
  as connected for a few seconds" and **Cancel**. It then becomes one of:
  - **Password needed** or **Wrong password**: a password field and
    **Continue**.
  - **Couldn't connect to *library***: why, and **Try again**.
  - The setup page, when the check succeeds.
- **Link *library*** (setup): an **Update read status in Calibre** toggle,
  "When enabled, books' completion and last-read date are synced to
  Calibre.", and once it's on, a **Read** and a **Last read** row, each
  opening a column picker. Then **Link**. If another library is linked, a
  dialog asks first: "Replace *old*? … Your books and their series stay on
  the Kindle. The rest of what *old* knew about them (tags, read status) is
  forgotten until *new* sends its own."
- **Column picker**: the library's Yes/No columns (for Read) or Date
  columns (for Last read) by their headings, and **Don't sync**. It
  explains how to add a column when there is none, and that Calibre lists a
  new column only after a restart. During setup it has **Check again**,
  which repeats the check and stays on the picker with what it found.
- **Home**: a box with Status, Linked to, Address, and "Syncing reading
  status to Calibre" or "Not syncing reading status". Then **Connect** or
  **Disconnect** and **Library settings**, then **Change library** and
  **Forget library**. Forget asks first: "Forget *library*? Your books will
  remain on the Kindle, and you can reconnect to this library later."
  - **Disconnect** pauses the service (`/pause`). It shows while the service
    is connected, connecting or waiting for Wi-Fi.
  - **Connect** shows otherwise. It resumes a paused service, or tries again
    at once (`/connect-now`), for example after Calibre wasn't found or
    ejected the Kindle.
  - When Calibre rejects the password, the box gets **Enter the password**.
- **Library settings**: the computer, address and password (**Change**),
  the read-sync toggle and pickers as in setup, and **Save**. The columns
  listed are those of the last connection.
- **Password**: a field, **Save** and **No password**.
- **Settings**, from a gear (⚙) on the first page:
  - **Start automatically** (the autostart sentinel, ADR 0010).
  - **Group books into series**: the `series` switch in `config.json`
    (ADR 0016), until now only editable by hand.
  - **Service: Restart and Stop.** The note below them says when the
    service starts again: "next time you open this app", plus "or restart
    the Kindle" when Start automatically is on.
  - The version.

**Layout rules.** No flexbox: rows use floats. A button alone on its row is
full width. A page with a back arrow has no Cancel button. Text scales from
the root font size as now (`innerWidth / 40`). The user-facing name for
unlinking is **Forget library**; the command line keeps `unlink`.

### Checking a library before linking it
A check is a short connection that links nothing and changes nothing on
the Kindle.

1. **Who answers.** The check needs the library's name and UUID, and
   whether Calibre wants a password.
   - **A library from Find** skips this step: Find libraries records the
     same things, including (new) whether a password challenge was set.
     That avoids a second failed connection counting towards the lockout.
   - **A library entered by address** goes straight to the full
     connection: its handshake says the same things. (Amended in the
     implementation, 2026-10-18: a separate probe first was one more
     connection counting towards the lockout. See the implementation
     notes.)

   If Calibre wants a password and none was given, the check ends here with
   "Password needed": for a found library before connecting, for an
   address at the start of the handshake.
2. **A full connection under a separate device ID.** The check connects as
   a device with its own ID: generated once, kept in the state directory,
   and reused by every check (developer, 2026-10-18). Calibre makes the
   ID at the first check, and the check keeps it. It never uses the
   Kindle's real ID, names no
   read-sync columns, and lists no books. It doesn't store the driveinfo
   Calibre sends (`SET_CALIBRE_DEVICE_INFO`). It reports the same device name
   as the real sessions.
3. **What it learns.** Whether the password is accepted (the handshake),
   and the library's columns with their headings and types
   (`SET_LIBRARY_INFO`).
4. **When it leaves.** At the first keep-alive after Calibre has been
   quiet for 3 s, the rule read sync uses to reconnect (ADR 0017) with a
   shorter wait, measured with the harness (implementation notes).
5. **If Calibre serves this Kindle right now.** Calibre serves one device
   at a time. If the service's own session is with the same Calibre, the
   service ends that session at a quiet keep-alive and holds off
   reconnecting until the check is over (linked, cancelled or failed).
   Other Calibre instances are unaffected.

What Calibre is left with: one empty cache file for the check's device ID,
reused by every check from this Kindle, and a device with no books shown
for a few seconds.

The result lives in the daemon's memory, like found libraries (ADR 0018):
the checked library's UUID, name, address, the password that worked, and
its columns.

**Find libraries remembers what it learned for 5 minutes** (developer,
2026-10-18). Each search still broadcasts, which Calibre answers from a
separate listener that doesn't count towards the lockout (`driver.py`,
the broadcast thread). It then connects only to servers it hasn't heard
from in the last 5 minutes. For the rest it reuses what it learned: the
library's name, UUID and whether it wants a password.
- **The list shows only servers that answered this search's broadcast.**
  A Calibre that has closed drops off at once.
- **The 5 minutes count from the last connection to that server,** so
  repeated searches don't extend a stale entry.
- **Search again uses the cache too.** Otherwise tapping it would bring
  back the risk.
- **A cached entry can be stale.** For example, Calibre may have switched
  to another library within the 5 minutes. The check's handshake names the
  library Calibre serves now. If that's a different library, the check
  says so and updates the entry, rather than linking the wrong one.
- **The cache is in memory.** It goes with the service, and on a Wi-Fi
  change.

**A lockout gets its own message.** When Calibre closes a connection before
sending anything, the check, Find libraries and the service's own session
report it as a probable lockout, not as "unreachable" or "not a Calibre
connection". The message names the fix: "Calibre may have stopped
accepting this Kindle after too many failed attempts. In Calibre, stop and
start the wireless device connection (Connect/share), then try again." The
exact wording comes with the build. This path must be tested with the
harness before release (Consequences).

### Linking with settings
- **Link** links the checked library with the settings chosen: `writable`,
  `read_column` and `last_read_column`. Replacing another library still
  asks first, and still resets as ADR 0018 describes (now keeping series).
- **The columns are the user's choice**, from the library's Yes/No and
  Date columns. ADR 0017's `#kindle_read` and `#kindle_last_read` become
  the columns picked by default when the library has them.
- **Changing settings later** (Library settings, Save) writes them and ends
  the session at the next quiet keep-alive, so the next session names the
  new columns (ADR 0017's reconnect).
- **The library's columns move into `config.json`** (developer,
  2026-10-18), on the linked library's entry, with their headings:
  `"columns": {"#kindle_read": {"name": "Kindle read", "datatype":
  "bool"}, …}`. `columns.json` goes.
  - **Linking** writes the checked library's columns along with the
    choices.
  - **Sessions** update them from `SET_LIBRARY_INFO` through
    `config::update`, the locked read-modify-write every writer uses (ADR
    0011, amendment).
  - **Forget and replace** drop them with the library, so the reset no
    longer has a file to clear.
  - **Library settings** reads the names and the choices from one place.

  ADR 0017 put them in a separate file, keyed by library UUID, when the
  allowlist could hold several libraries. With one linked library, they're
  a fact about that library. The cost: a little information learned from
  Calibre in a file people may edit by hand.
- **The command line** keeps `link HOST [PORT] [--writable]` and gains
  `--read-column` and `--last-read-column`, defaulting to ADR 0017's names.
  It doesn't run the full check.

### API (amends ADR 0011)
- **New:**
  - `POST /probe` with `{uuid}` (a found library) or `{host, port,
    password}`. It starts a check and returns at once.
  - `POST /probe/cancel`.
  - `POST /settings` with `{writable, read_column, last_read_column}`.
  - `POST /series` with `{enabled}`.
  - `POST /service` with `{action: "restart" | "stop"}`.
- **Changed:**
  - `POST /link` takes `{writable, read_column, last_read_column,
    replace}` and links the checked library. Without `replace`, linking
    over another library answers `needs_replace` as now.
  - `GET /status` gains `probe` (the check's state and result), `wifi`,
    `series`, and the linked library's `columns`.
- **Unchanged:** `/find`, `/unlink`, `/password`, `/pause`, `/resume`,
  `/connect-now`, `/reconnect` and `/autostart`.

### Service controls (amends ADR 0004)
ADR 0004 planned start and stop in the UI. The app gets **Restart** and
**Stop** instead: the API can't start a service that isn't running. The
app's tile starts it (the scriptlet, ADR 0004), as does autostart at boot.
How the daemon restarts its own upstart job (`initctl restart`, or exiting
and letting `respawn` start it) is an implementation choice. It must not
count towards the crash-loop guard (ADR 0010).

## Alternatives considered
- **Keep one long page**, with paging buttons, or with the app's sections
  collapsed. The developer asked for pages instead.
- **Set up read sync after the first real connection.** The first session
  would learn the columns, and a newly linked library is read-only by
  default, so nothing reaches Calibre before the user chooses. It's simpler,
  with no check and no second device ID. Rejected by the developer: a check
  also confirms the password and that Calibre is there before linking.
- **Check under the Kindle's own device ID**, listing no books: Calibre
  would empty its cache for this Kindle. Listing the real books instead:
  the stale-cache problem ADR 0018 avoids. Both rejected.
- **A new device ID for every check, or for every service start:** an
  empty cache file each time in Calibre's cache folder, where a fixed check
  ID leaves one. A new ID per service start was suggested as a way out of a
  lockout, but Calibre keys the lockout on the address, so it wouldn't
  help.
- **Leave right after `SET_LIBRARY_INFO`:** Calibre shows an error dialog.
- **Read columns some other way** (Calibre's content server, a plugin):
  the user's computer runs stock Calibre only (`docs/PLAN.md`).
- **From the review:**
  - A welcome page: unnecessary with "Choose a Calibre library" as the
    first page.
  - Unlink only, with no Change library: rejected.
  - A warning on the setup page instead of the replace dialog: the dialog
    won.
  - No service controls: the developer wanted them, in Settings.
  - "Disconnect temporarily": too long; plain Disconnect.

## Consequences
- **Work:**
  - A check mode for sessions, and the daemon running a check alongside
    its own session.
  - Find libraries recording whether a password is needed, and caching
    what it learned for 5 minutes.
  - The lockout message.
  - The API changes.
  - The columns moving into `config.json`.
  - The command-line options.
  - A new app built from the prototype's `app/` files, without the mock.
- **To test with the harness:**
  - How short the check's quiet wait can be.
  - What Calibre shows during a check: a device, and no dialog.
  - A check with the wrong password, and with none.
  - **The lockout path:** five failed connections (wrong passwords, or
    Find libraries without linking), then the message on the next check,
    Find and session, and that stopping and starting Calibre's wireless
    connection clears it.
  - Repeated searches within 5 minutes connecting to each server only
    once, and a library switched in Calibre within that time.
  - A check while the service is connected to the same Calibre.
  - Calibre starting a job during a check, say a send.
  - The check's cache file being reused.
- **To check on the device:**
  - That every page fits and the text is big enough.
  - Whether the Kindle's fonts have the ⚙ character.
  - How much of each form the keyboard covers.
- **Risks:**
  - A check adds a device to Calibre's view, briefly.
  - Each failed connection (a Find libraries probe, a wrong password)
    counts towards Calibre's lockout of 5 from one address, and the library
    list searches every time it opens. The 5-minute cache keeps searches to
    at most one connection per server in that time, and a completed check
    resets the count. The message above tells the user how to clear a
    lockout.
  - A cached library can be out of date for up to 5 minutes; the check
    catches a changed library before linking.
- **Docs:** the README's app section and the changelog.

## Implementation notes (2026-10-18)
Tested from the dev Mac against the harness Calibre (ADR 0019), with a
dev-only `check` command that runs the same code as the app's check
(`kindle-wireless-sync check --state DIR HOST [PORT] [--password PW]
[--quiet SECS]`).

- **A check works, and Calibre shows no dialog.** Calibre took the
  connection after about 5 s (its device polling), showed "Wireless device:
  Kindle" until the check left, then dropped it quietly. With the library's
  two custom columns, the check listed both with their headings and types.
- **The check's device ID.** Calibre assigned one at the first check
  (`8c4753a6-…`), the check kept it in `check_device_uuid`, and later checks
  reused it: Calibre's cache folder gained that one empty file and no more.
- **The quiet wait.** With the 10 s of sessions, a check took about 21 s.
  Leaving at the first keep-alive after the book list, three checks took
  3–7 s with no dialog; with a 3 s wait, 9–11 s. A check lists no books, so
  Calibre has nothing to follow the book list with. Checks wait 3 s as a
  margin; sessions keep 10 s.
- **The lockout, reproduced.** Five connections that read Calibre's first
  message and left made the next check end at once: Calibre accepted the
  connection and closed it without a word. The check reported
  `locked_out` with the message above, and Calibre showed no dialog of its
  own. Stopping and starting the wireless connection in Calibre cleared it,
  and the next check succeeded.
- **The separate probe was dropped.** With a password on the harness, a
  check by address cost two counted connections (the probe, then the full
  connection), so after a "Password needed" and a wrong password the right
  password hit the lockout. Without the probe, no password, two wrong
  passwords and then the right one all behaved: `password_needed`,
  `password_rejected` twice, then `ok`, with no dialog.
- **Not tested yet:**
  - A send started in Calibre during a check. The check ends with an
    error, which Calibre reports as a failed job.
  - A check while the service is connected to the same Calibre. A unit
    test covers it with a fake Calibre; it needs a real device.
