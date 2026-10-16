# 0016: Calibre series as the Kindle's native series grouping

- Status: Accepted
- Date: 2026-10-17

## Context
Phase 5: show Calibre series as the Kindle's own series grouping, with one
tile per series and the books inside in series order. The developer chose
native series over collections on 2026-10-16. The research is in
`research/kindle-library-integration.md`, "Native series on FW 5.20.1" and
"Making a sideloaded series show". In summary:

- **Interface:** the catalog's ChangeRequest endpoint,
  `POST 127.0.0.1:9101/change`, with an `AuthToken` header read from
  `/tmp/session_token`. It's the system's own path: the catalog's triggers
  keep `cc.db` and `ksdk.asset.db` in sync. That keeps ADR 0005's rule of no
  direct database writes.
- **What a visible series needs** (tested on the reference device):
  1. One `Series` row per book (`insertOr`, `table: SERIES`), with a seriesId
     of `urn:collection:1:asin-<KEY>`, where `<KEY>` is **exactly 10
     characters**.
  2. An `Entry:Item:Series` container (`insertOr`).
  3. `update` the container with `members: [book uuids]`, which sets the
     member counts the Library's filter needs (both at least 2), and with
     `isProcessed: 1`.
  4. `update` each book to `seriesState: 0` (hidden in the series) and
     `originType: 0` (otherwise the tile gets a Kindle Unlimited badge).
- **How it behaves:**
  - Replacing a book file in place keeps the series, but resets that book's
    `originType`.
  - Deleting a book makes the firmware's post-processing (run when the
    scanner notices the deletion) set the container's count to the number
    of *cloud* copies, which is always 0 for sideloaded books. It also
    un-hides **every** remaining member, however many are left. So the
    series falls apart until we re-apply it. It's a safe state, since books
    are visible and ungrouped, but the re-apply must come *after* the
    firmware has run. It also leaves the deleted book's `Series` row
    behind.
  - With grouping off (the default outside JP, US, UK and DE), the data has
    no visible effect: hidden members still show.
- Calibre already sends `series` and `series_index` with every book, and we
  store them in `books.json`. No Calibre setting is needed, unlike its
  collections map.

## Decision
**Which series:** we manage series built from our own `books.json` only:
books we received or adopted (ADR 0013) that have a Calibre `series` and a
content key (`_kws`). We never touch Amazon's series or books we don't
track. A series is created only when **2 or more** of its books are on the
device, since the Library ignores smaller ones anyway.

**Keys:** `<KEY>` is a fixed function of the series name: `"KW"` followed by
8 base-32 characters of the SHA-1 of the series name (trimmed and
case-folded). That gives 10 uppercase letters and digits, unlikely to clash
with a real ASIN (`B0…`) or an ISBN-10. The same name always gives the same
series.

**Container:**
- title = series name
- credits = the first book's authors
- thumbnail = the first book's cover
- `virtualCollectionCount` = the number of members, so the series view says
  "N out of N"
- `originType -1`, `isArchived 1`, `cdeType series`,
  `mimeType application/x-kindle-series` (as Amazon series and
  kindle-series-manager have)

**Members:**
- `itemPosition` = Calibre's `series_index` (books are ordered by it)
- `itemPositionLabel` = the index as written ("2", "2.5")
- `seriesOrderType ordered`

**Reconcile, don't patch.** We compare the series we want (from
`books.json`) with the series we last applied, which we record in
`state/series.json`. Then:
- **New or changed series:** write `Series` rows, the container,
  `members`, and `isProcessed`, **then** hide the members (`seriesState 0`,
  `originType 0`). Books are only hidden once a visible container exists.
- **Series that dropped below 2, or a book that left a series:** un-hide
  those books first (`seriesState 1`), then remove its rows, the links
  (`members` update) and, if it's empty, the container.
- **Firmware drift:** re-assert the member updates for every managed
  series, since a re-index resets `originType`, and a delete resets counts
  and visibility. All of these updates are idempotent.

**When to reconcile: a debounce after book changes.** Sessions can stay
open for hours, so we don't wait for one to end.
- Every change to our book list (received, replaced, deleted, metadata
  updated, adopted) marks the series state dirty and restarts a timer of
  about **15 s**. That's long enough for the scanner to index new files and
  for the firmware's post-processing after deletes, so we re-apply after
  the firmware rather than before it. A batch from Calibre arrives as one
  burst, so it's reconciled once.
- While a session is open, the session loop checks the timer whenever
  Calibre's keep-alive `NOOP` arrives (about every 3 s), and reconciles
  between Calibre's operations, never during a transfer. No extra thread is
  needed.
- When a session ends while the state is dirty, the daemon reconciles
  straight away, and it also reconciles once at start-up.
- If a new book has no catalog entry yet (not yet indexed), its series is
  left for the next pass instead of failing.

Commands go in batches of at most 100 (LibrarianSync's limit, to avoid
running out of memory).

**Failure handling:** if the endpoint or the token is unavailable (as off a
Kindle, or on firmware without it), we log once and skip. Books still sync,
just without series. A failed reconcile is retried after the next session.

**Uninstall** (`uninstall.sh`, not on upgrade) first runs
`kindle-wireless-sync series --remove-all`, which dissolves every managed
series so books aren't left grouped by a removed app.

**Switch:** on by default. It's harmless where grouping is off, and useful
where it's on. A config option (`series: false`) dissolves our series and
stops managing them. A UI toggle can come later.

## Alternatives considered
- **Writing `p_memberCount` directly with SQL** (kindle-series-manager):
  rejected by ADR 0005.
- **Collections:** deferred to the Later list. They work everywhere, but
  aren't what the developer wants for series.
- **Fake cloud copies** (`isArchived 1` entries), so the firmware's own
  post-processing would count members: rejected. Phantom cloud books would
  offer downloads that can't work, and a cloud sync might remove them.
- **Patching incrementally per event:** reconciling against our recorded
  state is simpler, and it repairs firmware drift as it goes.
- **Reconciling only when a session ends:** sessions can stay open for
  hours, so series would lag far behind the books.
- **Reconciling straight after each operation:** after a delete it would
  run before the firmware's post-processing, which would then undo it.

## Consequences
- The `members` update writes link rows in the catalog's `Collections`
  table for our series containers. Real collections aren't affected: a
  book's collection count only counts `Collection` entries, which was
  checked on the device.
- Member books get `originType 0`, the value for purchased books. Any other
  effects on sideloaded books are untested; watch for them.
- Series only appear where grouping is on. On AU and other unsupported
  marketplaces that needs the developer tool (`just series-grouping`) or
  the guarded script on the Later list.
- New code: a small HTTP POST client (we already speak HTTP for the API, so
  no new dependency), read-only `cc.db` lookups for `p_uuid` and `cdeKey`,
  the reconcile logic with tests against a fake endpoint, and a `series`
  command.
- Still to verify on the device, and to document:
  - that a series survives a reboot with its counts set
  - that nothing syncs our series or link rows to Amazon (their asset tags
    are `PENDING_SYNC`, like real series)
  - ~~what the Library does when a book is deleted while the user is
    viewing its series~~: done, see "Device test" below.

## Implementation notes (2026-10-17)
- **No `state/series.json`.** Each pass reads the catalog (read-only): our
  containers (keys matching `KW` + 8 base-32 characters), their `Series`
  rows, and the books' `seriesState` and `originType`. It plans only the
  commands that differ. This can't drift from what the catalog holds, a
  settled library sends nothing, and it never targets rows that don't
  exist. That matters because the endpoint rejects a whole batch when a
  delete or update matches nothing.
- **Container uuids** are derived from the key (name-based, version 5
  style), so they're stable.
- **When passes run:** `Library` records when series became dirty: on
  receive, replace, delete, adoption, and metadata updates that change a
  book's series or index (Calibre resends metadata on every connect). A pass
  runs once 15 s have passed since the last change, either on Calibre's
  keep-alive `NOOP` during a session or in the daemon's idle waits between
  sessions. A pass with a member not yet indexed, or one that fails, stays
  dirty and is retried.
- **Off switch:** `"series": false` in `config.json`. **Uninstall** runs
  `kindle-wireless-sync series --books … --catalog … --remove-all`.

## Device test (2026-10-17)
On the reference device, with grouping turned on and Calibre 9.16.0:
- **Four books sent from Calibre** (a 4-book series, sent in one batch):
  they showed individually for a few seconds. **15 s after the last one**,
  a single pass applied 10 changes (4 `Series` rows, the container, its
  member update and 4 hidden members), and the Library merged them into one
  series tile without a manual refresh. The tile had the first book's
  cover, a "4" badge and no Kindle Unlimited badge. Opened, it showed
  "You have 4 out of 4 items in this series", with books 1–4 in order.
- **Deleting a book from Calibre while its series view was open on the
  Kindle:** the view was **not** bounced back to the Library. It stayed open
  and updated live. About 9 s after the delete it showed "(3 items)" and
  "3 out of 4" (the firmware's own update, with our stored size still 4).
  Our pass ran 17 s after the delete, made 5 changes (removed the orphaned
  `Series` row, updated the container, and re-hid the 3 remaining books
  the firmware had un-hidden), and the view then read "3 out of 3".
- **A two-book series** (sent together): one pass 18 s after the send made
  6 changes, and the tile showed a "2" badge and "2 out of 2", in order.
- **Deleting one of the two while the series view was open:** within 6 s
  the firmware had un-hidden the remaining book and set the container's
  count to 0, and the open view read "(1 item)" and "1 out of 2". Our pass
  ran 15 s after the delete and made 5 changes: it un-hid the book, removed
  both `Series` rows, emptied the container's members and deleted it. That
  **did** bounce the open view back to the Library, where the remaining
  book showed as a plain book, with no stack or badge. So a series view
  closes only when its container is removed. While the container still
  exists, the view updates in place.
