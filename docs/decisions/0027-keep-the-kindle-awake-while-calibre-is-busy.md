# 0027: Keep the Kindle awake while Calibre is busy

- Status: Accepted (developer, 2026-11-20)
- Date: 2026-11-20

## Context
Wi-Fi goes down when the screensaver starts (ADR 0008), and the session
goes with it. The screensaver comes after 10 minutes without a touch, and
receiving books isn't a touch, so a long send from Calibre can be cut off
partway if the user puts the Kindle down (Later 14, developer,
2026-10-21). The interrupted book is lost, and Calibre shows an error.

powerd offers two ways to keep the device awake:

- **`preventScreenSaver`** (`lipc-set-prop -i com.lab126.powerd
  preventScreenSaver 1`, as `just screensaver-off` does). KOReader's
  `plugins/keepalive.koplugin` uses it. It's one global flag, not a count:
  powerd's `status` shows it as `prevent_screen_saver:0|1`.
- **`touchScreenSaverTimeout`**, which resets the inactivity timer as a
  touch would. KOReader's `frontend/device/kindle/powerd.lua`
  (`resetT1Timeout`) uses it, and notes that the value is irrelevant and
  that it fails once the screensaver is already showing.

**Checked on the reference device** (5.20.1, 2026-11-20): powerd's
`status` read `Remaining time in this state: 350.3` before
`lipc-set-prop -i com.lab126.powerd touchScreenSaverTimeout 1` and `599.9`
just after, so the call resets the timer to the full 600 s.

## Decision
While a session with the linked library is busy, reset the idle timer
with `touchScreenSaverTimeout`.

- **Busy** means Calibre sent something other than a keep-alive: books,
  deletes, metadata, book lists, the handshake. The session already tells
  these apart to decide when Calibre is quiet (ADR 0022); it now also calls
  `Admit::busy` for each.
- The daemon resets the timer **at most every 30 s**
  (`KEEP_AWAKE_EVERY`), and only once the linked library has been
  admitted. A library that isn't linked, and the library check (ADR 0020),
  never keep the device awake.
- `kindle::reset_screensaver_timer` runs the `lipc-set-prop` call and
  ignores failures. Off a Kindle (no lipc), the daemon is given a no-op.

## Alternatives considered
- **Set `preventScreenSaver` for the length of a send, and clear it
  after.** Rejected. It must be cleared however the session ends, and if
  the daemon crashes or is killed mid-send, the Kindle never sleeps until
  a reboot. Because it's a single flag, clearing it also undoes anyone
  else's setting (KOReader's keep-alive, or `just screensaver-off`).
- **Keep the device awake for the whole connection.** Rejected: Calibre
  can stay connected for hours, sending only keep-alives, and the user
  expects the Kindle to sleep then.

## Consequences
- Nothing to undo: when the resets stop (a send ends, the session drops,
  the daemon crashes), the screensaver comes on 10 minutes later as usual.
  Other apps' `preventScreenSaver` is untouched.
- A single book that takes longer than 10 minutes to arrive could still
  be cut off, since the timer is only reset between messages. At Wi-Fi
  speeds that's a book of several hundred megabytes.
- Once the screensaver is showing the call fails, but by then Wi-Fi is
  already down and the session over.
- The device stays awake up to 10 minutes after the last book, just as
  after a last touch.
