# 0008: Daemon connection behaviour and status

- Status: Accepted
- Date: 2026-10-15
- Amended: 2026-10-15 (see "Amendment" below; it replaces "Avoiding unwanted
  libraries")
- Amended by [0018](0018-one-linked-library.md) (2026-10-17): the daemon
  connects only to the one linked library, not to any allowed library.
  Password failures are tracked for that library alone. Libraries found by
  Find libraries are kept in memory, not in the config.

## Context
ADR 0004 runs the client as a long-lived upstart job that waits for Wi-Fi.
On the reference device, Wi-Fi goes down when the screensaver starts
(`cmState NA`, no default route), and the session dies with it. Calibre can
eject the device at any time.

The library UUID that the allowlist is keyed on (ADR 0003) is only known
after connecting. Calibre counts connections that don't complete a
handshake, per peer IP. After `MAX_UNSUCCESSFUL_CONNECTS` (5) it refuses
that IP with "Too many connection attempts" until a connection succeeds
(`smart_device_app/driver.py`, `detect_managed_devices`). Each refused
handshake may also show an error in that Calibre's UI.

The developer chose the behaviours below on 2026-10-15.

## Decision
**Main loop:** wait for Wi-Fi, look for an allowed Calibre, run a session,
then repeat according to how the session ended.

| Situation | Behaviour |
|---|---|
| Wi-Fi down | Wait for `com.lab126.wifid` `cmStateChange "CONNECTED"`. If Wi-Fi drops during a session, shut the socket down at once instead of waiting for a TCP timeout. |
| Wi-Fi up, no allowed Calibre answers | Retry discovery after 10 s, 30 s, then every 2 min. |
| Calibre ejects the device | **Stay disconnected until Wi-Fi cycles** (goes down and comes back, e.g. after sleep), or until the user presses **Connect now**. |
| Session ends otherwise (stream closed, error) | Retry using the backoff above. |
| Device sleeps | Nothing special. Wi-Fi drops, and we reconnect when it comes back. We never defer suspend. |
| Calibre busy (another device connected) | Retry using the backoff. |
| Password rejected | Stop trying that library until its settings change or Wi-Fi cycles. |

**Avoiding unwanted libraries:** a discovered server whose library is not
allowed is recorded in the config's `seen` list, with its host and port.
The daemon doesn't reconnect to a host:port known to serve a non-allowed
library until Wi-Fi cycles. This keeps us under Calibre's attempt limit and
stops error noise in other people's Calibre. Servers whose hostname matches
an allowed library's last-seen hostname are tried first.

**Status:** the daemon keeps a status the Phase 4 UI shows: a state plus a
human-readable reason, and when it started. States: waiting for Wi-Fi,
searching, no Calibre found (with the next retry time), connected (library
name), ejected (waiting for Wi-Fi to cycle), password rejected, stopped, and
error (message).

**Config:** `/var/local/kindle-wireless-sync/config.json` holds `allowed`
(library UUID, name, last host/port/hostname, optional password) and `seen`
(libraries we found but haven't allowed, for the UI). Until the UI exists,
`kindle-wireless-sync allow <uuid>` and `config.json` are the interface.

## Alternatives considered
- Reconnecting straight after an eject: rejected, because it makes the eject
  pointless.
- Deferring suspend while connected: rejected, because of battery cost and
  going against how the device normally behaves.
- Reconnecting to non-allowed servers on every retry: rejected, because of
  Calibre's per-IP attempt limit and the errors it shows its user.

## Consequences
- "Connect now" and status need a control channel into the daemon. In
  Phase 3 it is in-process. Phase 4 exposes it over the localhost API.
- A Calibre that switches to an allowed library on a host:port we recorded
  as non-allowed is only noticed after Wi-Fi cycles or on "Connect now".
- On the host there is no lipc, so the network monitor is pluggable and
  treats the network as always up there.

## Amendment (2026-10-15): find libraries on request; add servers directly

Calibre's discovery replies carry no library identity. The UDP reply has
only the short hostname and ports (`driver.py` ~L130). The mDNS record has an
empty property set (~L2031). The UUID only arrives after connecting. Two
things the developer reported or decided: the refused connection in Phase 2
produced no error or popup in Calibre, and the behaviours below.

- **The background loop only connects to allowed libraries.** Candidates are
  (a) discovered servers whose broadcast hostname or address matches an
  allowed library's last-seen hostname or host, and (b) each allowed
  library's last known `host:port`, tried directly, so libraries outside the
  broadcast domain work too. The UUID is still checked after connecting
  (ADR 0003). If a different library answers at an allowed address, it is
  recorded in `seen` and that address is skipped until Wi-Fi cycles. A
  Calibre that has never been allowed is never contacted in the background.
  With an empty allowlist the status says so and the loop just waits.
- **Find libraries** (a UI action, and `kindle-wireless-sync find` until the
  UI exists) runs discovery and connects once to each server not already
  matched to an allowed library. It reads `currentLibraryUUID` and
  `currentLibraryName` from the first message, disconnects without replying,
  and records the result in `seen` for the user to approve.
- **Add server** takes a host, port and optional password and is
  **synchronous**. The daemon connects to that address immediately. If it
  can't connect, or the first message isn't a Calibre handshake, the user
  is told why and **nothing is stored**. If it succeeds, the library's UUID
  and name are read and the library is **added to the allowlist** with that
  address, as if it had been found and approved.

This replaces "Avoiding unwanted libraries" above. With no background
connections to non-allowed libraries, the per-address skip list is only
needed for the mismatch case.
