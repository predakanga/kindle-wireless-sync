# Decision Log

One file per decision, named `NNNN-short-title.md`, numbered in sequence.
Don't edit a decision after the fact to change it. Write a new one that
supersedes it, and set the old one's status to `Superseded by NNNN`.

## Template

```markdown
# NNNN: Title

- Status: Proposed | Accepted | Superseded by NNNN
- Date: YYYY-MM-DD

## Context
What problem or question prompted this, and what research/evidence applies
(link sources: Calibre file/line, KOReader plugin, device observations).

## Decision
What we chose.

## Alternatives considered
What else was on the table and why it lost.

## Consequences
What this commits us to, and any follow-up work or risks.
```

## Index

- [0001: Target device and firmware](0001-target-device-and-firmware.md)
- [0002: Run lifecycle](0002-run-lifecycle.md)
- [0003: Calibre trust model and allowlist](0003-calibre-trust-model.md)
- [0004: UI, daemon and service layout](0004-ui-architecture.md)
- [0005: Register books through the system scanner](0005-library-registration-via-scanner.md)
- [0006: Crate structure and I/O model](0006-crate-structure-and-io-model.md)
- [0007: Accept every format the built-in reader opens](0007-accepted-formats.md)
- [0008: Daemon connection behaviour and status](0008-daemon-behaviour.md)
- [0009: Development commands behind a `dev-tools` feature](0009-dev-tools-feature.md)
- [0010: Autostart sentinel and crash-loop guard](0010-autostart-and-crash-loop-guard.md)
