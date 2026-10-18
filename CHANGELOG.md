# Changelog

All notable changes to Kindle Wireless Sync. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). The release workflow publishes
each version's section as its GitHub release notes, so every release needs
one.

## [Unreleased]

## [0.4.0] - 2026-10-19

### Added
- Choose the folder new books go into, inside the Kindle's `documents`
  folder: **Storage folder** in the app's settings opens a folder picker,
  which can also create folders. Books already on the Kindle stay where
  they are.

### Changed
- A book you send again from Calibre is updated in place, even if you
  renamed it in Calibre or changed the storage folder since, so its reading
  progress is kept.

## [0.3.2] - 2026-10-18

### Changed
- Quitting or restarting Calibre no longer leaves the Kindle disconnected.
  Wireless Sync notices that Calibre has closed and connects again when
  it's back, even on a new port. Ejecting the Kindle in Calibre still keeps
  it disconnected until Wi-Fi reconnects or you tap **Connect**. This also
  works for a Calibre on another network, linked by address, unless a
  firewall blocks Calibre's discovery ports.
- Disconnecting from the app, forgetting the library, changing its
  password or linking another library now waits for Calibre to finish
  what it's doing (15 seconds at most), so Calibre no longer shows an
  error if it was in the middle of something. Home shows "Disconnecting…"
  meanwhile. Reconnecting after a change of sync settings, a library check
  or a library switch in Calibre leaves Calibre more safely too.
- The app's Home shows why the Kindle isn't connected directly under the
  status, with what to do: for example "Ejected by Calibre. Tap Connect
  below to reconnect."

## [0.3.1] - 2026-10-18

### Fixed
- Books deleted from Calibre now leave the Kindle's library at once, keeping
  their covers until they go. Before, the Kindle removed them one at a time
  over 40 seconds or more for 8 books, showing them without covers
  meanwhile.

## [0.3.0] - 2026-10-18

### Added
- The Kindle can send each book's read status and last-read date to your
  Calibre library, into a Yes/No and a Date column you create
  and choose in the app. It's off by default: turn it on when linking a
  library, or later under **Library settings** (on the command line, `link
  --writable`, with `--read-column` and `--last-read-column`). See the
  README.

### Changed
- The **Wireless Sync** app is new: each page fits one screen, with no
  scrolling. Linking a library first tests the connection (and asks for
  Calibre's password if it has one), then lets you set up read status
  sync, choosing from the library's own columns. The gear opens settings:
  start automatically, group books into series, and restart or stop the
  service.
- While Calibre can't be reached, the app shows "Library not available"
  with a countdown to the service's next attempt.
- If Calibre stops accepting the Kindle after too many failed connections,
  the app says so and how to fix it (stop and start Calibre's wireless
  device connection). Finding libraries no longer adds to that count when
  repeated within 5 minutes.
- The Kindle is linked to one Calibre library, instead of an allowlist of
  libraries. Choose it from the libraries the app finds, or by address, or
  use `link` on the command line, which replaces `allow` and `add-server`.
  Linking a different library asks first, keeps your books and their series
  on the Kindle, and forgets the rest of what the old library knew about
  them. **After upgrading, link your library again.**

### Fixed
- Books sent as KFX now get Calibre's cover and join their series. The
  Kindle takes about 15 seconds to index each book, and longer when several
  arrive at once, so the app now fills these in once the Kindle has indexed
  the book, instead of giving up after 10 seconds. A transfer no longer
  pauses on each KFX book, and a series appears once all its books are
  indexed.
- Switching libraries in Calibre while the Kindle was connected kept the
  Kindle connected to the new library, so it synced with a library it
  wasn't linked to. The Kindle now disconnects, and the app says which
  library Calibre is serving. Switch back in Calibre, then tap **Connect**.
- The **Wireless Sync** app could fail to open ("The selected application
  could not be started") when the package was installed from a shell with
  a restrictive umask, because its files were readable only by root.
  Installing or upgrading now sets the files' permissions itself.
- After an upgrade, reopening the **Wireless Sync** app could show the old
  version, because the app stayed open in the background. Upgrading now
  closes it.

## [0.2.0] - 2026-10-16

### Added
- Books in a Calibre series are grouped as one series tile in the Kindle
  library, in series order, like series bought from Amazon. This needs the
  Kindle's "Group series" feature, which Amazon only offers in some
  countries.
- Uninstalling removes the series this app created.

## [0.1.1] - 2026-10-16

### Fixed
- The **Wireless Sync** tile in the Library has its icon again. The 0.1.0
  package was built without it.

### Changed
- Calibre now shows the Kindle's own name (*Settings → Device Options →
  Device Name*) instead of "Kindle". A rename on the Kindle shows from the
  next connection.

## [0.1.0] - 2026-10-15

First release.

### Added
- Receives books from Calibre's wireless device connection and adds them to
  the Kindle's built-in library as soon as each transfer finishes, with
  covers. Calibre converts books to formats the built-in reader opens
  (AZW3, MOBI, AZW, KFX, PDF, TXT).
- Deleting books from Calibre, and replacing a book in place, which keeps
  the reading position.
- Only Calibre libraries you allow can connect. Find libraries or add a
  server by address, with an optional password.
- The **Wireless Sync** app in the Kindle library: connection status, pause
  and resume, Connect now, allowed libraries, and an option to start the
  service automatically when the Kindle starts.
- Books that Calibre sent over USB are recognised, so Calibre shows them as
  on the device instead of sending them again.
- Install, upgrade and uninstall through KPM. Uninstalling keeps your books.

[Unreleased]: https://github.com/predakanga/kindle-wireless-sync/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/predakanga/kindle-wireless-sync/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/predakanga/kindle-wireless-sync/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/predakanga/kindle-wireless-sync/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/predakanga/kindle-wireless-sync/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/predakanga/kindle-wireless-sync/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/predakanga/kindle-wireless-sync/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/predakanga/kindle-wireless-sync/releases/tag/v0.1.0
