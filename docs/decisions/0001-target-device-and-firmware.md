# 0001: Target device and firmware

- Status: Accepted
- Date: 2026-10-15

## Context
Kindle models and firmware versions differ in CPU ABI, content catalog, and
UI framework. We need a reference platform to develop and test against.

## Decision
The primary target is a **Kindle Paperwhite 12th Gen on firmware 5.20.1**,
the developer's own device. Every base requirement must work there. Other
models and firmware versions are supported on a best-effort basis.

## Alternatives considered
- Supporting a wide range of firmware from the start: rejected. Without
  hardware to test on, it would slow down the base requirements.

## Consequences
- Phase 1 research answers questions for 5.20.1 first. It notes where
  behaviour is known to differ on other firmware, but doesn't chase those
  differences.
- An extra stretch phase at the end of the plan checks whether the on-device
  integration points we rely on exist on other firmware versions.
