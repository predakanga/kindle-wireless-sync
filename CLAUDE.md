# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

A Rust homebrew package for jailbroken Kindles, installed through KPM. It
receives books from Calibre's **Wireless device connection** and adds them
straight to the Kindle's **built-in** reader. It does not target KOReader or
any other third-party reader.

Hard constraints (from the requirements in `docs/PLAN.md`):
- Rust only, packaged for KPM.
- The user's PC runs stock Calibre and nothing else. A Calibre plugin is
  allowed only for the stretch goals (Series→collections, progress/rating
  sync back to Calibre).
- An on-device UI lets the user allowlist Calibre connections.
- Books appear in the Kindle library as soon as a sync finishes.

## Current status

Phase 1 (research) of `docs/PLAN.md`. There is no code yet. **Once a Cargo
workspace or justfile exists, add the build, test, single-test and
deploy-to-device commands here.**

## Working process

- **Plan, then research, then build.** `docs/PLAN.md` holds the phases, exit
  criteria and open questions. Keep it current: tick off phases and move
  answered questions into decisions.
- **Record design decisions** as ADRs in `docs/decisions/` (template and index
  in its README). Write one whenever a choice affects architecture, the
  protocol, the on-device integration or the packaging. Cite evidence such as
  Calibre source paths or device observations.
- **Commit as you go**, in small commits at each meaningful step (a research
  finding written up, a decision recorded, a feature working). Don't batch a
  whole phase into one commit.

## Key references

- Calibre smart device driver (the server side of the wireless protocol):
  `src/calibre/devices/smart_device_app/` in https://github.com/kovidgoyal/calibre
- KOReader's `calibre.koplugin`, an existing client implementation of the
  same protocol: https://github.com/koreader/koreader/tree/master/plugins/calibre.koplugin
- Kindle Button Mapper, the model for the Rust Kindle GUI app, KPM packaging
  and the justfile debug/deploy workflow:
  https://github.com/zampierilucas/kindle-button-mapper-rs
