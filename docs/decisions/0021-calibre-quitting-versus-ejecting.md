# 0021: Telling Calibre quitting from an eject

- Status: Accepted
- Date: 2026-10-18
- Amends: [0008](0008-daemon-behaviour.md) (an eject keeps
  the service disconnected) and [0020](0020-one-screen-pages-and-checking-a-library.md)
  (the Find cache).

## Context
**An eject keeps the service disconnected** until Wi-Fi cycles or the user
taps Connect (ADR 0008). That respects a user who ejects the Kindle in
Calibre. But quitting Calibre looks the same, so the Kindle stays
disconnected when Calibre comes back:
- Restarting Calibre, which adding a column requires, left the Kindle
  disconnected (developer, 2026-10-17; `PLAN.md`, Later item 8).
- In the app review, the developer thought they had to tap Connect while
  Calibre was away (2026-10-18).

**What Calibre does** (Calibre 9.16.0, `smart_device_app/driver.py` and
`gui2/ui.py`):
- **A user's eject** (`umount_device`) calls the driver's `eject()`: a
  `NOOP` with `ejecting: true`, then it closes the device's socket. The
  wireless connection keeps running: Calibre still listens for devices and
  still answers discovery broadcasts.
- **Quitting** calls `stop_plugin('smartdevice')`, which calls the same
  `eject()` and then `_shutdown()`. That closes the listening socket and
  the broadcast socket. *Stop wireless device connection* does the same.
  So the message is identical; what differs is that afterwards Calibre no
  longer answers broadcasts.
- **Broadcast replies name the port.** Calibre answers a datagram on any of
  its broadcast ports (54982, 48123, 39001, 44044, 59678) with its
  hostname, its content server's port and its device port. It answers from
  its listener thread, so a broadcast never counts towards the lockout of
  5 failed connections (ADR 0020).
- **The port can change.** Without *Use fixed network port*, Calibre takes
  9090 if it's free and otherwise a random port (`_startup_on_demand`). A
  restart usually keeps the port, but not always.

## Decision
**After an eject, ask whether Calibre is still there, by broadcast.**
1. Wait 2 s, so a quitting Calibre has closed its sockets.
2. Send the discovery datagram to the Calibre computer's address on each
   broadcast port, and wait up to 2 s for an answer from that address.
   Send it twice, in case one is lost.
3. **An answer means a user's eject.** Stay disconnected, as ADR 0008
   says, until Wi-Fi cycles or Connect.
4. **No answer means Calibre quit,** or stopped its wireless connection.
   Wait for it to come back:
   - Repeat the broadcast every 10 s for 5 minutes, then every 60 s. Use
     the normal broadcast too, so a Calibre that came back at another
     address is found as well. Nothing connects until Calibre answers, so
     waiting costs no lockout attempts.
   - Home shows "Library not available", with the countdown to the next
     broadcast, and the advice "Calibre has closed. Wireless Sync
     reconnects when it's back."
   - When Calibre answers, connect to the port in its answer, as for any
     discovered server. A session that succeeds records the address and
     port in `config.json`, as now.
5. **When Calibre couldn't be asked, keep ADR 0008's rule.** If the linked
   library's Calibre has never answered a broadcast since Wi-Fi came up
   (for example, it's on another network and was linked by address, or a
   firewall drops the datagrams), silence proves nothing. An eject then
   keeps the service disconnected, as now, rather than ignoring a user's
   eject.

**Calibre coming back with another library** is handled as now (ADR 0018):
the handshake names it, it joins the found list, and the status says to
switch back.

**The Find cache follows the port** (amends ADR 0020). The cache is keyed
by address and port, so an answer on a new port is already a new entry,
connected to once. Two additions keep it from serving stale answers:
- When the service decides Calibre at an address has quit, it drops that
  address's cache entries, whatever their port.
- When a broadcast answer from an address names a different port from its
  cached entries, those entries go.

## Alternatives considered
- **Reconnect after every eject.** Simple, but it overrides a user who
  ejected the Kindle on purpose, which ADR 0008 chose to respect.
- **Try a TCP connection to Calibre's port after the eject.** After a quit
  it's refused, which costs nothing. But after a user's eject Calibre
  accepts it, and an unfinished connection counts towards the lockout.
  Rejected for the broadcast, which never counts.
- **Tell them apart by timing** (a quit closes the connection right after
  the eject). Fragile: nothing in Calibre promises the timing.
- **Ask the user** after each eject whether to reconnect. Nobody is looking
  at the Kindle when Calibre restarts.

## Consequences
- **Work:** the daemon's eject handling gains the broadcast check and a
  "Calibre closed" wait (a new outcome, shown as `not_found` with a
  countdown and advice); discovery gains a unicast send to one address;
  the Find cache gains the two clean-ups.
- **To test with the harness:**
  - Quit and relaunch Calibre (with its wireless connection started again):
    the service waits, then reconnects without Connect.
  - Eject from Calibre: the service stays disconnected.
  - Stop and start the wireless connection: like a quit.
  - A different port on relaunch (the fixed-port option, or another
    program holding 9090): the service connects to the new port and
    `config.json` records it.
- **To check on the emulated Kindle:** whether a datagram sent to the Mac
  (10.0.2.2) gets an answer through QEMU's network. Broadcasts don't cross
  it. If unicast doesn't either, the emulator always takes rule 5.
- **Later item 8** in `PLAN.md` is answered by this ADR once it's built.
