# 0006: Crate structure and I/O model

- Status: Accepted
- Date: 2026-10-15

## Context
Phase 2 builds the Calibre protocol client so it runs and is tested on the
host, before any Kindle integration. The protocol is one TCP connection at a
time, where the server sends a request and the device replies, with raw book
bytes inline. The final binary has to be small and static for armv7 musl
(ADR 0004).

## Decision
- **One Cargo package**, `kindle-wireless-sync`, with a library and a binary.
  We'll split into a workspace only if a second binary or a Calibre plugin
  needs it.
- **Naming:** everything a user or the system sees uses the full name
  `kindle-wireless-sync`: the package, the binary, the KPM id, the upstart
  jobs, the install directory `/var/local/kindle-wireless-sync/` and the log
  files. Calibre shows our `appName` "Kindle Wireless Sync". The developer
  found the initialism hard to parse. Code-internal names stay short: the
  library crate is `kws` (`use kws::session`) and our temp files are
  `.kws-*.part`.
- **Blocking `std` I/O, no async runtime.** There's one Calibre session at a
  time. Discovery and the localhost API can use their own threads.
- Layering inside the library:
  - `protocol`: opcodes and length-prefixed JSON framing over any
    `BufRead + Write`. The JSON is kept as `serde_json::Value` with typed
    accessors, because Calibre's metadata dicts are large and change between
    versions.
  - `lpath`: validation of server-supplied paths (ADR 0003).
  - `library`: on-disk state, meaning the books directory, our book list and
    `driveinfo`. All writes go through the ADR 0005 file pattern (temp file
    plus rename, in-place rewrite for updates). Kindle-specific steps (cover
    thumbnails, EXTH parsing) are added in Phase 3 as hooks inside `library`.
  - `session`: the device side of one Calibre session. It's generic over the
    stream and the library, so tests can drive it with a scripted fake
    Calibre over an in-memory or localhost stream.
  - `discovery`: UDP broadcast discovery.
- Minimal dependencies: `serde_json` (dynamic `Value` only, **no `serde`
  derives**), `log` + `env_logger` (default features off), `sha1_smol`
  (password hash), `lexopt` (CLI) and `libc` (`statvfs`). Dev-only:
  `tempfile`.
- **Binary size is tracked** with `just size-kindle`. A new dependency needs
  a measured size cost.

### JSON crate measurement (2026-10-15)
Tiny armv7-musl release binaries (`opt-level="z"`, LTO, strip, panic=abort)
that parse a Calibre-style frame, then edit and re-serialize it:

| Binary | Size | JSON crate adds |
|---|---|---|
| baseline (std + musl, stdin → stdout) | 312,608 B | n/a |
| `serde_json` 1 (`Value`, no derives) | 408,016 B | +95 KB |
| `json` 0.12 | 400,120 B | +88 KB |
| `tinyjson` 2 | 383,000 B | +70 KB |

`serde-lite` was also considered (the developer suggested it). It cuts
**derive-generated** code, but it serializes through an intermediate value
that still goes through `serde_json` for JSON I/O. With no derives in our
code it would add a layer and save nothing. `serde_json` costs only ~25 KB
more than the smallest option and is the most robust. `tinyjson` stores all
numbers as `f64`, so opcodes and lengths could be written as `0.0`.
- Tests replay **scripted Calibre conversations** taken from recordings of
  real sessions. The binary can log every frame it sends and receives (but
  not book bytes) to a transcript file for this.

## Alternatives considered
- tokio/async: more code and a bigger binary, for concurrency we don't need.
- `clap`: much larger than `lexopt` for a CLI with a few flags.
- Typed structs for all of Calibre's metadata: brittle across Calibre
  versions. We only type the fields we act on.

## Consequences
- Long transfers block the session thread. That's fine, because Calibre
  waits for each reply anyway.
- Our stored book records round-trip Calibre's JSON as-is, apart from
  stripping the thumbnail. A side effect is that Calibre may resend metadata
  for books it has no cache for, which is harmless.
