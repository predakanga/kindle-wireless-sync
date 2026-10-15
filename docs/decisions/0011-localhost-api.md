# 0011: Localhost API for the on-device UI

- Status: Accepted
- Date: 2026-10-15
- Amended: 2026-10-15 (config ownership; see below)

## Context
ADR 0004 puts the UI in a WAF app that talks to the daemon over a localhost
HTTP JSON API. The UI needs: status (ADR 0008, including why we're not
connected), Connect now, pause/resume, the autostart toggle and whether the
crash-loop guard turned it off (ADR 0010), Find libraries, Add server,
approving and forgetting libraries, and per-library passwords.

The reference WAF apps (Kindle Button Mapper) use ES5 `XMLHttpRequest` with
`Content-Type: text/plain`, which avoids CORS preflight, and the server
answers with `Access-Control-Allow-Origin: *`. Any web page open in the
Kindle's browser can reach `127.0.0.1` the same way. For us that would let
a page add an attacker's Calibre to the allowlist (ADR 0003).

## Decision
- The daemon serves HTTP/1.1 on **`127.0.0.1:8325`**, one request per
  connection, from a thread inside the daemon process. It is a small
  hand-written server, because an HTTP crate would cost binary size (ADR
  0006). Requests are capped at 16 KB.
- **Origin check:** any request whose `Origin` header is an `http://` or
  `https://` origin gets 403. The WAF app is loaded from `file://`, so
  browsers send `null`, `file://` or nothing. Responses carry
  `Access-Control-Allow-Origin: *` so the WAF app can read them. The check,
  not CORS, is what keeps web pages out.
- Bodies are JSON sent as `text/plain`. Replies are JSON. A user-level
  failure (for example an unreachable server in Add server) is
  `200 {"ok": false, "error": "..."}`, so the UI can show the message. A
  malformed request is 400.
- Endpoints:

| Method | Path | Body | Effect |
|---|---|---|---|
| GET | `/status` | | State, detail and since (ADR 0008); `paused`; `autostart`; `autostart_disabled_by_guard`; allowed libraries (with `has_password`, never the password itself); seen libraries; version |
| POST | `/connect-now` | | As ADR 0008 |
| POST | `/pause`, `/resume` | | Pause disconnects and stops syncing until resumed. The process and API keep running (ADR 0004) |
| POST | `/autostart` | `{"enabled": bool}` | Creates or deletes the sentinel. Enabling also clears the guard marker and counter |
| POST | `/find` | | Find libraries (ADR 0008 amendment). Synchronous, a few seconds. Returns what answered |
| POST | `/add-server` | `{"host", "port", "password"?}` | Synchronous probe. Allows the library only on success |
| POST | `/allow` | `{"uuid"}` | Seen → allowed |
| POST | `/forget` | `{"uuid"}` | Removes an allowed library. Cuts the session if it is the connected one |
| POST | `/password` | `{"uuid", "password": string or null}` | Sets or clears a library's password. Clears a previous "password rejected" |

- **Pause** is a daemon state (`paused`), not an upstart stop, so the UI can
  always reach the API. It isn't persisted: a restart or reboot (with
  autostart) resumes syncing.
- `config.json` is changed only through `config::update`: an exclusive
  `flock` on `state/config.lock`, then load, change and save. The change
  itself goes through `Config`'s typed operations (`allow_seen`, `forget`,
  `set_password`, `record_seen`, …), so the API can only make the changes its
  endpoints name. See the amendment below.

## Alternatives considered
- A custom request header to force a CORS preflight: may break requests
  from `file://` in mesquite, and we can't easily test every firmware's
  engine.
- A shared secret: the WAF app has no secure place to read one from.
- A Unix socket: WAF apps can only make HTTP requests.

## Consequences
- Local processes can still use the API. That's acceptable: anything
  running locally on a jailbroken Kindle already has root.
- The `Origin` value mesquite actually sends must be checked on the device
  when the WAF app is first tested.

## Amendment (2026-10-15): who owns `config.json`

The developer asked whether the UI should send update messages to the
backend rather than having it read-modify-write JSON. It already does: each
endpoint maps to one typed operation. What was open was how the writers
coordinate. The writers are the daemon loop (on every connection), the API
thread (user actions), and CLI processes (development and headless use).
The options considered:

- **A. The file is the source of truth, with locked load-modify-save.**
  Works with or without the daemon running; hand edits are always safe. The
  in-process lock didn't cover CLI processes.
- **B. The daemon owns an in-memory config, and the CLI becomes an API
  client.** One writer, but CLI management needs a running daemon, hand
  edits made while it runs are lost, and it needs an HTTP client in the
  binary. A mutex is still needed between the two threads.
- **C. An actor thread owns the config.** The same guarantee as B, with more
  boilerplate. The daemon loop can't be the actor, because it blocks inside
  sessions.

**Chosen: A, with an `flock`** (developer's decision). It closes the
cross-process gap with a few lines. `tests: config::concurrent_updates_are_not_lost`
fails without the lock. `/forget` and `/password` call `Handle::reconnect`
so the change takes effect at once.
