# 0025: Read the catalog with an embedded SQLite

- Status: Proposed
- Date: 2026-10-20
- Amends: [0006](0006-crate-structure-and-io-model.md) (a new dependency)
  and [0017](0017-read-status-sync.md) (how `cc.db` is read)

## Context
The binary reads `cc.db`, read-only (ADR 0005), by running the device's
`sqlite3`:
- `Catalog::query` (`src/catalog.rs`) runs `sqlite3 -readonly -json` and
  parses the JSON rows. Series (ADR 0016) and read sync (ADR 0017) use it.
- `lookup_indexed_key`, `lookup_entry_uuid` and `entry_listed`
  (`src/kindle.rs`) run `sqlite3 -readonly` and parse its default text
  output. They give the covers and KFX keys, and the deletes that leave
  the Library at once (ADR 0005).

The developer's compatibility testing (2026-10-20) found that many
firmware versions ship a `sqlite3` without JSON output. `-json` arrived in
SQLite 3.33.0, and the Basic 2016 on 5.16.2.1.1 has 3.26.0: there every
`Catalog::query` fails, so series and read sync never work (progress log,
phase 6). Every query also builds its SQL by quoting paths into it.

Kindle Series Enabler had the same problem with the firmware's `sqlite3`
and solved it by building SQLite in (its [ADR
0004](https://github.com/predakanga/kindle-series-enabler/blob/main/docs/decisions/0004-a-rust-binary.md)):
`rusqlite` with the `bundled` feature, cross-compiled with
`cargo-zigbuild`.

**Size, measured (2026-10-20).** The packaged binary (no `dev-tools`)
built with `cargo zigbuild --release --target
armv7-unknown-linux-musleabihf`:

| Binary | Size |
| --- | --- |
| Today's code | 905,636 bytes |
| With `rusqlite` 0.40 (`bundled`) opening and querying `cc.db` | 2,049,396 bytes |

So SQLite adds about 1.1 MB, and the binary stays well under the 5 MB
budget. (Today's `rust-lld` build is 887,608 bytes; Zig's linker adds
18 KB.) Series Enabler's binary, which also has SQLite, is 1.55 MB.

## Decision
**Embed SQLite: `rusqlite` with `bundled`**, as Series Enabler does. All
of the binary's database reads go through it, and the binary no longer
runs `sqlite3`.
- **One read-only query helper** in `catalog`: it opens the file with
  `SQLITE_OPEN_READ_ONLY` and returns rows as maps of column name to
  `serde_json::Value`, the shape `-json` gave. Series and read sync parse
  rows as before. The `kindle.rs` lookups use the same helper.
- **No busy timeout.** As with the CLI, a query fails at once with
  `database is locked` while the scanner holds `cc.db`. The callers
  already retry or try again later (ADR 0005), and a long wait could stall
  a reply to Calibre.
- **Values are bound as parameters** (`?1`), not quoted into the SQL.
- A connection is opened per query, as each CLI run was. Nothing holds
  `cc.db` open between queries.

**Build:** the Kindle builds use `cargo zigbuild` with Zig 0.17.0 and
`cargo-zigbuild` 0.23.4, as Series Enabler's do, because `bundled` needs a
C compiler for armv7 musl. `.cargo/config.toml` (the `rust-lld` linker
setup) goes. CI's `package` job and the release job install both tools.
Host builds and tests use the host's C compiler.

**Tests** build their fixture databases with `rusqlite`, so `cargo test`
and CI no longer need the `sqlite3` CLI.

**Shell keeps the CLI.** `install.sh` (waiting for the scriptlet's catalog
entry), `register-app.sh` and the scriptlet (`appreg.db`) use plain
`sqlite3` with no JSON, which works on the old versions too. The
compatibility check (`tools/compat`) then only needs `sqlite3` with
`-readonly`.

## Alternatives considered
- **Keep the CLI and parse its text output** (`-csv`, or the default list
  mode, which old versions have): no new dependency and no size cost. But
  titles and paths can hold the separator and newlines, so the parsing is
  ours to get right. We'd also still depend on whatever `sqlite3` each
  firmware ships, and on its options and output.
- **Link the device's `libsqlite3.so`:** a static musl binary can't, and a
  dynamic build would tie the binary to each firmware's libraries (Series
  Enabler's ADR 0004).
- **Embed SQLite only where `-json` is missing:** two code paths, and the
  one used on the reference device wouldn't be the one that needs testing.

## Consequences
- The binary grows by about 1.1 MB, to about 2 MB.
- The Kindle build needs Zig and `cargo-zigbuild` on the Mac and in CI. The
  developer already has both (Zig 0.17.0) for Series Enabler.
- Our SQLite and the firmware's share `cc.db` through SQLite's
  usual file locks, as any two SQLite builds do. We still never write it
  (ADR 0005).
- Old firmware no longer fails at `sqlite3 -json`. Other differences may
  remain: the Basic 2016's `Entries` has no `p_lastOpenTime` (read sync),
  and it's a soft-float device that isn't supported (ADR 0001).
- **To verify:** series and read sync on the reference device, and the
  catalog queries on a firmware with an old `sqlite3` (`just compat`).
