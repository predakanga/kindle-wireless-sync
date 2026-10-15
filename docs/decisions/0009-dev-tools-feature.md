# 0009: Development commands behind a `dev-tools` feature

- Status: Accepted
- Date: 2026-10-15

## Context
The binary has user-facing commands (`daemon`, `find`, `add-server`,
`allow`, `status`) and development commands:
- `discover`: broadcast only, no TCP connection. A network diagnostic.
- `connect`: one foreground session with a transcript option. Used for
  host testing and recording replay fixtures.

`discover` and `find` look alike in the help text, and the developer asked
for the development commands to be hidden by default. They are run **on the
Kindle** during development (`just device-connect`, `just device-discover`),
which needs a release-optimised armv7 build.

## Decision
- A cargo feature **`dev-tools`**, off by default, compiles in `discover` and
  `connect` (a `dev` module in `main.rs`) and a "Development commands"
  section of `--help`. Without it, those commands don't exist and help lists
  only the user commands.
- The development `just` recipes (`build`, `build-kindle`/`deploy`,
  `connect`, `discover`) build with `--features dev-tools`. The package
  (`build-kindle-package`, and Phase 4's KPM build) builds without it.
  `just check` runs clippy both ways.
- Library code those commands use (transcripts, `discovery`) stays in the
  library unconditionally. It is small and the tests use it.

Size on armv7 musl (2026-10-15): 656,048 B without the feature and 668,464 B
with it (+12 KB).

## Alternatives considered
- Tying the commands to `cfg(debug_assertions)`: rejected, because the
  commands are needed in release builds on the device.
- Hiding them behind `--help-debug` but always compiling them in: rejected,
  because the package would still carry them.

## Consequences
- The packaged binary is smaller, and its command list matches what users
  need.
- Anyone reproducing a bug report with `connect` on a user's device needs a
  dev build.
