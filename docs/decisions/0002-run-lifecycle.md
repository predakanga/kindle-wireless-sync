# 0002: Run lifecycle

- Status: Accepted
- Date: 2026-10-15

## Context
The sync client needs network access and must be running for Calibre to see
the Kindle. Running all the time costs battery. Running only on demand adds
friction.

## Decision
- The user can **start and stop** the sync service from the on-device UI.
- The user can opt in to **automatic start** at boot and/or when the network
  comes up, if the platform allows it.
- kindle-hid-passthrough (https://github.com/zampierilucas/kindle-hid-passthrough)
  already does this, so it is the reference implementation.

## Alternatives considered
- Always-on service: rejected. Users should control battery and network use.
- On-demand only: rejected. Users who sync often would have to start it by
  hand every time.

## Consequences
- The binary is split into a long-running service and a UI that controls it.
  Phase 1 has to find out how kindle-hid-passthrough registers its service
  (upstart or init) and how it detects network-up.
- The UI shows service state, not only connection state.
