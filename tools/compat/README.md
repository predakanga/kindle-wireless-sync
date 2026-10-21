<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Compatibility checks on emulated Kindles

`compat.py` checks whether a Kindle model and firmware version has what
Kindle Wireless Sync relies on, on an [eink-emulator](../../../eink-emulator)
instance (PLAN Phase 6): the platform, the tools, and the firmware APIs, each
probed directly. It doesn't install or run the package, which keeps the
emulator's own flakiness out of the results (progress log, Phase 6). It
boots the emulated Kindle, checks the prerequisites, runs every check, and
prints **one JSON object on stdout**. Progress goes to stderr.

```sh
just compat kindle-paperwhite-6 5.20.1            # appends to compat.jsonl
tools/compat/compat.py kindle-paperwhite-6 5.20.1 >> compat.jsonl
tools/compat/compat.py --list                     # the checks
```

Models are eink-emulator's ids (`../eink-emulator/eink models`); versions
are its release names (`./eink versions --model MODEL`). Exit status: 0 if
every required check passed, 1 if one failed, 2 if a prerequisite or the
boot failed, 130 if interrupted.

## What it does

1. **Prerequisites (on the Mac):** the emulator checkout, model and
   firmware, SSH key and free ports. It warns if a real
   Kindle is on USB or Tailscale manages DNS: either breaks the emulator's
   Wi-Fi (CLAUDE.md).
2. **The instance:** `kws-<short model>-<version>`, e.g. `kws-pw6-5201`,
   or `--instance NAME`. A missing one is created with `--jbsh` (Paperwhite 2
   and later) or `--jailbreak` (Kindle 4, Touch, Paperwhite 1), downloading
   the firmware if needed. Its first boot runs without `-snapshot` so jb.sh
   can jailbreak it (about 5 minutes on a Paperwhite 6). **An instance the
   run created is deleted when it exits** (`./eink delete`, which also drops
   base images nothing else uses), including after a failure, Ctrl-C or
   `kill`. Pass `--keep-instance` to keep it, so later runs skip the
   jailbreak. Instances that already existed are never deleted.
3. **The test boot** uses QEMU's `-snapshot`, so the checks never change the
   instance's disk. It waits for SSH, then for Home and the catalog's
   session token.
4. **The checks** (`checks.py`), in order:

| Check | Required | What it looks at |
|---|---|---|
| `device_info` | no | `/etc/version.txt`, kernel, screen, free space |
| `arch_kindlehf` | yes | `/lib/ld-linux-armhf.so.3`, and which KPM build jb.sh installed |
| `jailbreak` | yes | KPM is there |
| `commands` | yes | every command the package scripts and binary run |
| `sqlite3` | yes | `-readonly`, for the package scripts (the binary has its own SQLite, ADR 0025); records whether `-json` exists |
| `lipc` | yes | `wifid cmState`, `appmgrd activeApp`; the device name and `powerd` are optional |
| `paths` | yes | `documents/`, `system/thumbnails`, `/var/local`, `/var/log`, `/etc/upstart` |
| `catalog_schema` | yes | every `Entries` and `Series` column `src/` reads, and those the series commands set |
| `appreg_schema` | yes | the tables `register-app.sh` writes |
| `upstart` | yes | `framework_ready` and the `filesystems` job |
| `catalog_endpoint` | yes | ChangeRequest on `127.0.0.1:9101` with the session token |
| `scanner_indexing` | yes | a TXT copied into `documents/` is indexed, and `ccat updated` fires |
| `subfolder_indexing` | no | a TXT in a subfolder is indexed like one at the top (ADR 0023) |
| `series` | yes | a two-book native series made and dissolved through ChangeRequest, with the commands `src/series.rs` sends |
| `catalog_delete` | yes | ChangeRequest `delete` removes the entries, and the scanner doesn't re-add them |
| `wifi_events` | no | Wi-Fi off and on gives `cmStateChange` events |
| `ui_pages_fit` | yes | every page of the app fits on one screen (below) |
| `ui_pages_fit_keyboard` | yes | the field being typed in stays above the on-screen keyboard |
| `ui_browser` | yes | the browser features the app uses |

Statuses are `pass`, `fail`, `warn`, `skip` (an earlier check it needs
failed) and `error` (the check itself broke). A model is `compatible` when
no required check failed or errored. The only writes are TXT test files in
`documents/` (named `kws-compat-*`), a test series in the catalog through
ChangeRequest (both removed again), and the fit check's test app in
`appreg.db`; the snapshot boot discards them anyway.

## Choosing what to run

The package targets kindlehf devices only, so firmware 5.16.3 and later
(5.16.2.1.1 and older are soft-float). Two working assumptions narrow the
runs: what the firmware offers (tools, catalog columns, APIs) depends on
the version, not the model; and whether pages fit depends on the model, not
the version. Neither is exact (page fit may vary a little between
versions), but they find problems quickly.

- **One run per version, every model at least once:** `fingerprint.py
  --plan` prints the fewest runs that cover every kindlehf version
  eink-emulator can download and every model with one. With 40 versions and
  12 models (2026-10-20), that's 40 runs: the versions set the floor.
- **One run per fingerprint group:** `just compat-fingerprint` reads each
  version's root filesystem without booting it (`fingerprint.py`, with
  e2fsprogs' `debugfs`) and records what the checks depend on: hard-float,
  the commands, the catalog columns, the ChangeRequest field map in ccat's
  `change.lua`, the app registry tables in `libappreg`, the upstart jobs and
  the WebKit build. Versions with the same `features` should give the same
  results, so one run per group covers them, plus one per model for page
  fit. Firmware not yet imported is downloaded, read and deleted again.
  Results go to `fingerprints.jsonl` (gitignored); `info` and `files` record
  what differs without changing the group (the `sqlite3` build, schema
  versions, file hashes).
- What a fingerprint can't see: the running system (indexing, the catalog
  endpoint, Wi-Fi events). Those are assumed to follow the files.

**Result (2026-10-20): two groups.** Every kindlehf version but 5.19.3
(Amazon's download now answers 403 for every model) was fingerprinted:

| Group | Versions | Differs by |
|---|---|---|
| 1 | 5.16.3 to 5.18.6 (29) | no `Entries.p_lastOpenTime` (read sync, ADR 0026) |
| 2 | 5.19.1 to 5.20.1 (10) | everything present |

Within each group the files still differ between releases (ccat's Lua,
`libappreg`, upstart jobs, WebKit builds, and the `sqlite3` CLI: 3.26.0 up
to 5.18.1.5, 3.47.0 from 5.18.1.1.1), so a group is a working assumption,
not proof. **The runs that cover both groups and every model: 12**, each
model on its newest firmware, which is what most of its users run:

| Model | Firmware | Group | Result (2026-10-21) |
|---|---|---|---|
| KT4, Paperwhite 4 | 5.18.1.1.1 | 1 | no `p_lastOpenTime`; the rest passes |
| Oasis 3 | 5.18.2.1.1 | 1 | no `p_lastOpenTime`; the rest passes |
| Paperwhite 5 | 5.19.2 | 2 | passes |
| Basic 2022 | 5.19.2.0.1 | 2 | passes |
| Paperwhite 6, Basic 2024, Colorsoft, Scribe 1 and 2 | 5.20.1 | 2 | passes |
| Scribe 3 | 5.20.1 | 2 | pages fit (by hand); the rest untested: the emulator doesn't get past account setup |
| Scribe Colorsoft | 5.20.1 | 2 | untested, as Scribe 3 |

The Paperwhite 6 also has runs on 5.17.1.0.2 (group 1) and 5.19.2. Timings
and details are in the [progress log](../../docs/progress-log.md#phase-6).

## The page-fit check

`fitcheck/harness.js` runs the `package/ui` files in a test WAF app
(`com.kindlewirelesssync.fitcheck`, in `/var/local/kws-fitcheck`). Loaded
before `script.js`, it replaces `XMLHttpRequest` with a fake service and
walks the app to one page per scenario: every page and dialog, with long
names and full lists (34 scenarios). For each it measures the page against
mesquite's viewport and reports to `fitcheck.py` through an SSH reverse
tunnel (guest port 18326). The host takes a screenshot, then the page
reloads for the next scenario.

Typing scenarios focus a field, which opens the on-screen keyboard. mesquite
doesn't shrink the viewport for it, so the host finds the keyboard's top
edge in the screenshot (the first row dark across the full width) and
checks that the field is above it.

Add a scenario to `SCENARIOS` in `harness.js` when the app gains a page or
a state that changes a page's height.

## Output

One JSON line:

```json
{"schema":1,"model":"kindle-paperwhite-6","firmware":"5.20.1","instance":"…",
 "package":{"version":"0.4.0","commit":"…","dirty":false},
 "outcome":"completed","boot":{"baseline":false,"ssh_s":41.2,"home_s":120.5},
 "prereqs":[{"id":"…","status":"pass",…}],
 "checks":[{"id":"arch_kindlehf","title":"…","status":"pass","required":true,"detail":"…","data":{…},"duration_s":1.2}],
 "device":{…},"summary":{"counts":{"pass":19},"blocking":[],"compatible":true},
 "artifacts":"target/compat/…", …}
```

`outcome` is `completed`, `aborted` (a prerequisite failed),
`create_failed`, `boot_failed` or `interrupted`. `instance_removed` says
whether the run deleted the instance it created. The artifacts directory (default
`target/compat/<instance>-<time>/`) has `result.json` (the same, indented),
the QEMU logs and the screenshots (`fit-NN-<page>.png`).

## Options worth knowing

- `--instance NAME`: use an existing instance, such as `pw6-5201-jb`. Leave
  the developer's own `pw12` alone.
- `--attach`: run the checks against an instance that's already running on
  `--ssh-port`, without booting or stopping it. Its disk keeps the changes
  unless it was started with `-- -snapshot`.
- `--only ID,…` / `--skip ID,…`: a subset. Some checks need earlier ones
  (`series` needs `scanner_indexing` and `subfolder_indexing`) and are
  skipped without them.
- `--keep-instance`: keep an instance this run created. `--keep-running`
  keeps it too, since a running instance can't be deleted.
- `--no-snapshot`, `--artifacts DIR`.

The emulator is less reliable when the Mac is busy: a one-off failure is
worth a rerun before reading anything into it.

Only one run per instance at a time. Different instances can run at once:
ports move up from 4022/4023 when those are taken.
