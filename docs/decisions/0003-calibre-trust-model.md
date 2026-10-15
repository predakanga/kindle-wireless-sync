# 0003: Calibre trust model and allowlist

- Status: Accepted
- Date: 2026-10-15
- Amended: 2026-10-15. How libraries get onto the allowlist is refined in ADR
  0008's amendment: "Find libraries" lists them for approval, and "Add server"
  probes an address and allows the library that answers. Entries stay keyed by
  library UUID.

## Context
In the wireless protocol, Calibre is the TCP server and the Kindle connects to
it. Once connected, Calibre controls the session: it writes and deletes files
and reads device files (`GET_BOOK_FILE_SEGMENT`). The optional Calibre
password only proves the *device* to Calibre (`sha1(password + challenge)`).
Calibre never proves anything to the device. What identifies a Calibre
instance: the hostname and port from the UDP broadcast reply, the IP, and
`currentLibraryUUID` / `currentLibraryName` from the handshake. See
`docs/research/calibre-wireless-protocol.md`.

## Decision
- The device only **connects to allowlisted Calibre instances**. An entry is
  keyed by **library UUID**, and also stores the last-seen hostname, IP and
  port to use as a reconnect hint.
- New instances are found by broadcast (and/or entered by hand) and shown in
  the UI as **pending**. The user approves one, and we connect for its first
  handshake. If the library UUID doesn't match the approved entry, we
  disconnect.
- Whatever the allowlist says, every server-supplied path is treated as
  hostile. Paths must be relative, with no `..` and no NUL, have an allowed
  extension, and resolve inside the books directory after `realpath`.
  `GET_BOOK_FILE_SEGMENT` only serves files we track.
- We support Calibre's password per entry, as a way for the user to stop
  *other* devices connecting to their Calibre. It doesn't protect the Kindle.

## Alternatives considered
- Allowlisting by IP or hostname alone: rejected. DHCP changes break it, and
  it is no harder to spoof than the library UUID.
- Connecting to any Calibre automatically (KOReader's behaviour): rejected,
  because the requirements ask for an allowlist.

## Consequences
- The allowlist stops accidental connections (a housemate's Calibre). It does
  **not** stop a hostile peer on the same LAN who knows the library UUID. The
  README must say so.
- The first connection to a new library needs a UUID check after the TCP
  connection is made, so the approval flow has two steps.
