# 0022: Leaving a session at a keep-alive, without replying

- Status: Accepted (developer, 2026-10-18)
- Date: 2026-10-18
- Amends: [0017](0017-read-status-sync.md) ("When to reconnect"),
  [0020](0020-one-screen-pages-and-checking-a-library.md) (a check leaving,
  and a session ending for a check or new settings) and
  [0018](0018-one-linked-library.md) (a library switch ending the session).

## Context
The service ends a session on purpose for several reasons. Some wait for a
quiet moment: new read-sync columns (ADR 0017), new sync settings and a
check of the same Calibre (ADR 0020), and Calibre switching library
(ADR 0018). Each replies to a keep-alive once Calibre has sent nothing else
for 10 s, then closes. The others cut the connection at once: Disconnect,
Forget library, a new password and linking another library, as well as
Wi-Fi going down and the service stopping. Dropping the connection while
Calibre runs a job shows its device error dialog. The developer found the
quiet-moment rule iffy (`PLAN.md`, Later item 10).

What Calibre does (9.16.0; `research/calibre-wireless-protocol.md`, "Ending
a session from the device"):
- **There's no way for the device to ask to be ejected.** Calibre only reads
  replies to its own requests.
- **The keep-alive is sent only between jobs.** Calibre's device thread
  takes turns: it detects the device (sending the keep-alive, a `NOOP` with
  an empty payload), runs every queued job, then sleeps 2 s. When idle, it
  sends a keep-alive about every 10 s.
- **A keep-alive that fails is taken as the device leaving.**
  `detect_managed_devices` catches the error, and the device is removed as
  if it had dropped off Wi-Fi, with no dialog.
- **Jobs still queued then are aborted** ("Device no longer connected."),
  which shows the dialog. These are jobs queued in the 2 s since the last
  turn's jobs ran. The device can't see them.
- **Ending a session doesn't count towards Calibre's lockout** of 5 failed
  connections. Calibre resets an address's count when its handshake
  succeeds (`open`), so only connections that end before then count.

## Decision
**1. End at a keep-alive by not replying to it.** Where the service ends a
session at a quiet keep-alive, it now closes the connection instead of
replying, then reconnects as before. Calibre has already removed the device
when it gets the failure, so no job can start on a closed connection
afterwards. Before, a job queued just before the keep-alive ran after our
reply and failed part-way. Now it's aborted before it starts. The same goes
for a library check leaving (ADR 0020).

The 10 s quiet rule stays (3 s for a check). Straight after a job, Calibre
sends keep-alives on every probe, while its callbacks may be about to
queue the next step (for example `sync_booklists` after an upload).
Leaving then would abort that step. The reasons that wait for quiet are
the Calibre-side ones: new columns appear at the start of a session, and
switching library sets off jobs. For the Kindle-side ones (settings, a
check), Calibre has usually been idle for longer anyway.

**2. The user's Disconnect leaves gracefully.** Disconnect, Forget library,
a new password and linking another library no longer cut the connection at
once. The session ends at Calibre's next keep-alive, without the quiet
rule and without replying, because the user asked. If no keep-alive comes
within 15 s (`LEAVE_WITHIN`), for example during a long upload, the service
cuts the connection, as before. Meanwhile Home shows "Disconnecting…" (state
`disconnecting`). Wi-Fi going down and the service stopping still cut at
once.

## Alternatives considered
- **Reply, then close (before this).** A job queued in the 2 s before the
  keep-alive runs against the closed connection and fails part-way, and
  one queued just after our reply fails at Calibre's next probe.
- **Close at any moment, not at a keep-alive.** If Calibre is mid-job, the
  job fails.
- **Drop the 10 s quiet rule, now that leaving is cleaner.** The jobs at
  risk are exactly those queued around the first keep-alives after a job.
- **Leave at once for Disconnect.** That's what the user asked for, but it
  can fail a running send part-way. Waiting up to 15 s turns most of those
  into clean aborts or no failure at all.

## Consequences
- The remaining risk is a job queued in the 2 s before the keep-alive we
  leave at: Calibre aborts it with its error dialog. That can't be seen
  from the device.
- Disconnect can take up to about 10 s while Calibre is idle (its keep-alive
  interval), and 15 s at most.
- `SessionEnd::Left` and `Admit::leave_now` are new. The session tests now
  check that the keep-alive the session leaves at goes unanswered.
- **To test with the harness:** a column change, a settings change and a
  check, all ending without a dialog and reconnecting; and Disconnect
  while idle and during a send.

## Implementation notes (2026-10-18)
Against the harness Calibre (9.16.0), with the service on the Mac:
- **Keep-alive timing, measured:** every 2 s for about 5 s after a request,
  then every 10 s, as the driver's code says. `LEAVE_WITHIN` (15 s) covers
  one idle interval with a margin.
- **Disconnect while idle:** "Disconnecting…", then the session left at the
  next keep-alive (10 s later) without replying. No dialog.
- **Disconnect just before a send:** Disconnect was tapped before Calibre
  started sending a book. Calibre ran the whole send (free space, the book,
  the book list, free space), and the session left at the first keep-alive
  after it. The book arrived, and there was no dialog and no job left.
- **New sync settings:** the session left at the next quiet keep-alive and
  reconnected 5 s later, naming the new columns (Calibre's driver had
  `#kindle_read` and `#kindle_last_read`).
- **A check of the same Calibre:** the session left at a keep-alive, the
  check ran and left at its own keep-alive without replying, and the
  session reconnected 4 s later.
- **Calibre switching library:** the session left 16 s after the switch, at
  the first keep-alive after 10 s of quiet. Home said Calibre was serving
  the other library.

No case showed a dialog. A job queued in the 2 s before the keep-alive
wasn't tried; it can't be timed from outside Calibre.
