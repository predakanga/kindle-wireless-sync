# Handoff (2026-10-17): one-off read, not a living document

**This file is a one-off.** It hands one session's context to the next
agent. Don't update it or keep it going: progress lives in `docs/PLAN.md`
and the ADRs, and durable working notes in `CLAUDE.md`. Once you've read it
and the developer agrees, delete it in its own commit. (The last handoff
went the same way, in `8560264`.)

Start with `CLAUDE.md`, then `docs/PLAN.md`. The developer's working
preferences are also in your memory files.

## Where things stand

- **Released:** v0.2.0 (series grouping, ADR 0016) is on GitHub and in the
  KPM index.
- **`main` is 22 commits ahead of `origin/main`, unpushed.** Don't push or
  tag without asking (`CLAUDE.md`, "Working process").
- **Built and device-tested, unreleased: read sync (ADR 0017, Accepted).**
  It sends read status and last-read date from the Kindle to Calibre, into
  the user-created columns `#kindle_read` and `#kindle_last_read`, for
  libraries marked `writable` (off by default). It's set with `allow
  --writable` / `--read-only`. `CHANGELOG.md` has an Unreleased entry.
  - **The device test found three bugs, all fixed** (ADR 0017,
    "Implementation notes"):
    1. reconnecting while Calibre was mid-job showed an error dialog;
    2. Calibre's `"None"` dates made it skip a book's sync silently;
    3. unanswered `DISPLAY_MESSAGE`s hung Calibre's device thread.
  - All 100 tests and `just check` pass.
- **Not to release yet.** The developer wants ADR 0018 settled before the
  next version (it would have been v0.3.0).

## In progress: ADR 0018 (Proposed), link the Kindle to one Calibre library

`docs/decisions/0018-one-linked-library.md`. It replaces the allowlist with
a single **linked** library (the word is "link"; "pair" was rejected).
Agreed with the developer so far:
- **Config:** one optional `library` object, holding the read-sync
  settings, plus `series`. No migration, since the app has never been
  shared. The developer's Kindle is re-linked by hand.
- **Found libraries** (the old `seen` list) live in the daemon's memory,
  not the config.
- **Command line:** `link HOST [PORT] [--uuid UUID]` replaces `allow` and
  `add-server`; `--uuid` refuses a different library. `unlink` replaces
  Forget. `find` only prints.
- **API:** `/link` and `/unlink` replace `/allow`, `/add-server` and
  `/forget`.
- **Linking a different library** asks first, then:
  - clears `driveinfo.json` and `columns.json`, and resets `writable`;
  - keeps each book record's identity: `lpath`, `size`, `last_modified`,
    the Calibre **UUID**, title, authors, and `_kws` minus its read-sync
    state;
  - drops the rest of the old library's metadata.

  We don't delete `books.json`, because adoption can't recover TXT books
  (and maybe not PDF or KFX ones). Backed-up covers are kept.

**Next step, agreed but deferred to a session with the developer:** the
ADR's open question. Does clearing `driveinfo.json` make Calibre treat the
Kindle as a new device, with a fresh metadata cache? Proposed method:
1. Back up `/var/local/kindle-wireless-sync/state/driveinfo.json`, clear
   it, and reconnect.
2. Check Calibre's debug log and its device cache (see "Debugging Calibre"
   below).
3. Restore the backup afterwards unless the developer wants the change
   kept.

Then the developer accepts the ADR (it stays Proposed until they say so),
and it gets built. On acceptance:
- add "Amended by 0018" notes to ADRs 0003 and 0008;
- change ADR 0017's "each allowed library" to "the linked library";
- reword `PLAN.md`'s requirement 3 and its allowlist mentions.

## Device and Calibre state

- **The Kindle runs a development build** from `just deploy` (with
  `dev-tools`), not the KPM package. The service is running with
  autostart on, and the screensaver is on.
- **Calibre Library** (UUID `6d8cc30c-…`) is the only allowed library, now
  **read-only**. `columns.json` records no columns for it.
- **The developer deleted `#kindle_read` and `#kindle_last_read` in
  Calibre** after the removal test. The custom-columns list is now empty,
  so their research column `#read_progress_custom` is gone too. Mention
  it in case that wasn't intended.
- **On the Kindle:** "There Is No Antimemetics Division" is a real book
  the developer sent for the read-sync test; leave it. Several other books
  are marked read on the Kindle from the developer's end-of-book tests.

## Things learned this session (worth knowing before you test)

Some of these may belong in `CLAUDE.md` permanently. Suggest that to the
developer rather than doing it unasked.

- **Debugging Calibre:**
  - Have the developer quit Calibre, then run
    `/Applications/calibre.app/Contents/MacOS/calibre-debug -g` in the
    background with output to a file in the scratchpad.
  - The driver's `SMART_DEV` lines only reach the file when Calibre exits:
    `PYTHONUNBUFFERED` has no effect. Quit it with `osascript -e 'quit app
    "calibre"'` (it reports "User cancelled" but quits), then read the log.
  - Calibre's per-device metadata cache is
    `~/Library/Caches/calibre/wireless_device_<device_store_uuid>_metadata_cache.json`:
    length-prefixed JSON records (an 8-byte length line, then the record).
  - Read Calibre's database read-only with `sqlite3 -readonly
    "$HOME/Calibre Library/metadata.db"`. Custom columns are in
    `custom_columns`, with values in `custom_column_N`.
- **Calibre swallows sync errors.** `set_books_in_library` wraps
  `synchronize_with_db` in `except Exception: return True`, so a book can
  be skipped with nothing logged.
- **Quitting Calibre ejects the Kindle**, and the daemon then waits (ADR
  0008; Later item 8). To reconnect from SSH, run `curl -s -X POST
  http://127.0.0.1:8325/connect-now` on the Kindle. Connect now does
  nothing while a session is open: to get a fresh book list, restart the
  service (`just restart`).
- **Don't drop the connection while Calibre is mid-job.** Dropping it
  during an idle keep-alive is safe; at any other time Calibre shows an
  error dialog.
- **`RUST_LOG=debug` truncates long protocol messages** with "…". To run
  the daemon by hand with it, stop the service, then start the binary with
  the arguments from `service.sh`.
- **Never kill and start the daemon in one SSH command.** The new daemon's
  command line contains "kindle-wireless-sync daemon", so `pgrep -f`
  matches the SSH shell itself and kills it. Use two SSH calls.
- **Library menus change size.** A book's ⋮ menu can have extra items
  ("Search this book" appears after the book has been opened), so "Mark as
  read/unread" moves. Take a screenshot before tapping. The tile's ⋮
  button is at about (29%, 51%) for the first tile.
- **Probing read state:** a `kws-*` AZW3 built with `ebook-convert`, with
  the service stopped so adoption doesn't pick it up. Results are in
  `docs/research/kindle-library-integration.md`, "Read state probe".

## Added after the handoff was written (same day, a short interstitial session)

This handoff hadn't been read yet when a short session added two pieces of
test tooling. The developer asked for them to be noted here. **`main` is
now 32 commits ahead of `origin/main`**, still unpushed. Nothing above
changed: ADR 0018 is still Proposed, and its open question is still the
next step.

### Calibre test harness (ADR 0019, Accepted)

A development-only Calibre plugin and a client drive an **isolated**
Calibre from scripts: `just calibre launch|status|wireless-start|
wait-device|send|delete|field|device-books|dialogs|exec|quit`. Usage and
pitfalls are in `tools/calibre-harness/README.md`; `CLAUDE.md` points
there.
- It runs Calibre with its config, caches and library in `dev-calibre/`
  (gitignored). Your Calibre and `~/Calibre Library` are never touched. Only
  one Calibre GUI can run per user, so **ask the developer to quit theirs
  first**. The harness refuses to launch otherwise.
- The test library's UUID is `4d795e39-d508-4185-b4f6-1f449818b1bb`. It
  holds "KWS Harness Test" (TXT, id 1) and Calibre's Quick Start Guide
  (EPUB, id 2). It has **no custom columns**, so read-sync tests need
  `#kindle_read`/`#kindle_last_read` added first.
- `dialogs` lists open dialogs, error dialogs included, with their details.
  `wait-device` fails as soon as one opens.
- The ADR's implementation notes have a reliable way to drop the device
  mid-job: freeze it with `kill -STOP`, start a send, `kill -9` it. It
  reproduces ADR 0017's first bug (the "Error communicating with device"
  dialog).
- **When you use `just calibre exec`, add the snippet to the README's
  "`exec` snippets in use" list.** Ones that recur become named commands.

### Emulated Kindle (eink-emulator)

`../eink-emulator` runs Kindle firmware in QEMU. Instance **`pw6-5201-jb`**
matches the reference device: a Paperwhite 12th Gen on 5.20.1, jailbroken
with jb.sh. It's documented in `CLAUDE.md`, "Emulated Kindle". Read the
checklist there first.
- **State:** stopped (guest synced first). It has the 0.2.0 package built
  from this `main` (read sync included), installed with its own KPM, with
  autostart off. It's linked (allowed, since ADR 0018 isn't built) to the
  harness library at `192.168.15.201:9090` over its USB network. The
  `Kindle-QEMU` Wi-Fi profile is saved. One book is on it, "KWS Harness
  Test". A send through the harness reached its Library.
- SSH host `kindle-emu` (in the developer's `~/.ssh/config`). Device
  recipes take `KINDLE_HOST=kindle-emu`, and now use `scp -O`, which the
  emulator needs. Legacy scp was checked on the real Kindle too.
- **When its network misbehaves, check two things first:** is the real
  Kindle plugged in (its USB network overlaps the emulator's subnet), and
  is Tailscale managing the Mac's DNS? Either one breaks QEMU's DNS
  proxies, and `wifid` then never reaches CONNECTED. Ask the developer to
  change them; don't change them yourself.
- `find` can't see Calibre through QEMU (no broadcast). Link by address.

**Possibly useful for ADR 0018's open question:** the emulator and the
harness together might let the `driveinfo.json` test run without the real
Kindle. Back up and clear the emulator's `driveinfo.json`, reconnect, and
inspect Calibre's view with `device-books` or `exec`. Suggest this to the
developer rather than assuming it.
