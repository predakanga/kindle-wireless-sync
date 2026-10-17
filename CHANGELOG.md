# Changelog

All notable changes to Kindle Wireless Sync. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). The release workflow publishes
each version's section as its GitHub release notes, so every release needs
one.

## [Unreleased]

### Added
- The Kindle can send each book's read status and last-read date to your
  Calibre library, into two columns you create (`#kindle_read` and
  `#kindle_last_read`). It's off by default: turn it on with `link
  --writable`. See the README.

### Changed
- The Kindle is linked to one Calibre library, instead of an allowlist of
  libraries. Link it with **Find libraries** or by address in the app, or
  with `link` on the command line, which replaces `allow` and `add-server`.
  Linking a different library asks first, keeps your books on the Kindle,
  and forgets what the old library knew about them. **After upgrading, link
  your library again.**

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
