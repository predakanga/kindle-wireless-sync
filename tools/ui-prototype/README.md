<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# UI prototype

A clickable prototype of the Wireless Sync app with one screen per page,
for the UI review before 0.3.0. No Kindle, service or Calibre needed: run
`just ui-prototype`, or open `index.html` in a browser.

- **Left:** the app at the Paperwhite 12th gen's size (1272×1696; the app
  gets about 1272×1483 below the Kindle's bars, measured from a screenshot).
  The panel reports whether the current page fits without scrolling.
- **Right:** a fake service and fake Calibre libraries (`app/mock.js`).
  Presets set up common situations; each library can be switched off,
  hidden from Find, made busy, given a password or given columns.
- **Design variants** switch between alternatives still being decided.

The app (`app/`) is ES5 with no flexbox, like the real one, so it can be
copied to the Kindle to check the fit there. Opened on its own, it runs its
own fake service, starting with nothing linked.

The API it calls is a proposal: `/probe` connects to a library, checks the
password and reads its columns, then disconnects; `/link` links the probed
library with the chosen settings; `/settings` changes them later.
