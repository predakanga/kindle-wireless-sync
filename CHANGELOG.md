# Changelog

All notable changes to Kindle Wireless Sync. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). The release workflow publishes
each version's section as its GitHub release notes, so every release needs
one.

## [Unreleased]

### Added
- The Kindle can send each book's read status and last-read date to your
  Calibre library, into a Yes/No and a Date column you create
  (`#kindle_read` and `#kindle_last_read`, or others named with
  `--read-column` and `--last-read-column`). It's off by default: turn it on
  with `link --writable`. See the README.

### Changed
- The **Wireless Sync** app is new: each page fits one screen, with no
  scrolling. Linking a library first tests the connection (and asks for
  Calibre's password if it has one), then lets you set up read status
  sync, choosing from the library's own columns. The gear opens settings:
  start automatically, group books into series, and restart or stop the
  service.
- If Calibre stops accepting the Kindle after too many failed connections,
  the app says so and how to fix it (stop and start Calibre's wireless
  device connection). Finding libraries no longer adds to that count when
  repeated within 5 minutes.
- The Kindle is linked to one Calibre library, instead of an allowlist of
  libraries. Link it with **Find libraries** or by address in the app, or
  with `link` on the command line, which replaces `allow` and `add-server`.
  Linking a different library asks first, keeps your books and their series
  on the Kindle, and forgets the rest of what the old library knew about
  them. **After upgrading, link
  your library again.**

### Fixed
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

[Unreleased]: https://github.com/predakanga/kindle-wireless-sync/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/predakanga/kindle-wireless-sync/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/predakanga/kindle-wireless-sync/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/predakanga/kindle-wireless-sync/releases/tag/v0.1.0
