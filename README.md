# Kindle Wireless Sync

[![CI](https://github.com/predakanga/kindle-wireless-sync/actions/workflows/ci.yml/badge.svg)](https://github.com/predakanga/kindle-wireless-sync/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/predakanga/kindle-wireless-sync)](https://github.com/predakanga/kindle-wireless-sync/releases/latest)
[![Licence: GPL-3.0-or-later](https://img.shields.io/badge/licence-GPL--3.0--or--later-blue)](LICENSE)

Send books from Calibre to a jailbroken Kindle over Wi-Fi. They go straight
into the Kindle's own library, with no USB cable, no extra software on your
computer, and no reboot or rescan.

It uses Calibre's built-in **Wireless device connection**, so your computer
needs nothing but stock Calibre. The books land in the Kindle's built-in
reader. KOReader and other third-party readers are out of scope.

> [!WARNING]
> **Written with AI, and tested on one Kindle.**
>
> - This project was written using AI (Anthropic's Claude, through Claude
>   Code), directed and reviewed by its developer.
> - It has only been tested on the developer's personal **Kindle Paperwhite
>   (12th Generation) on firmware 5.20.1**. Other models and firmware
>   versions may work, but are untested.
>
> It modifies a jailbroken device. Use it at your own risk, and keep your
> books backed up in Calibre. It comes with no warranty (see
> [Licence](#licence)).

## Features

- **Wireless sending from Calibre.** Calibre sees the Kindle as a wireless
  device: send, replace and delete books as usual. Each book appears in the
  library as soon as its transfer finishes.
- **Allowlisted libraries.** The Kindle only talks to the Calibre libraries
  you allow, found on your network or added by address. Password-protected
  Calibre connections work too.
- **On-device app.** A **Wireless Sync** tile in the Library shows the
  status. It lets you connect or pause, manage libraries, and choose whether
  the service starts automatically when the Kindle starts.
- **Native series.** Books in a Calibre series are grouped into one series
  tile, in series order, like series bought from Amazon. This needs the
  Kindle's "Group series" feature, which Amazon only offers in some
  countries.
- **Books sent over USB.** Books that Calibre previously sent to the Kindle
  over USB are adopted, so Calibre shows them as being on the device.
- **Your Kindle's name** is the device name shown in Calibre.

Formats: AZW3, MOBI, AZW, KFX, PDF and TXT. The built-in reader doesn't open
sideloaded EPUBs, so Calibre converts them when sending.

## Requirements

- A jailbroken Kindle with [KPM](https://kindlemodding.org/jailbreaking/whats-next/installing-homebrew.html#kpm)
  (the Kindle Package Manager) installed.
- Calibre on a computer on the same network. A different network also works,
  using *Add a server by address* in the app.

## Install

With Wi-Fi on, in kterm or over SSH on the Kindle, add the repository once,
then install:

```sh
kpm add-repo https://predakanga.github.io/kindle-wireless-sync/repo.json
kpm update
kpm install kindle-wireless-sync
```

To upgrade, run `kpm update` and `kpm install kindle-wireless-sync` again.
Your settings are kept. Install from the repository rather than from a
downloaded `.kpkg` file, because KPM can't upgrade a package that was
installed from a file.

To uninstall, run `kpm uninstall kindle-wireless-sync`. Books you received
stay in your library. The series this app created are removed.

## Use

1. In Calibre, choose *Connect/share → Start wireless device connection*.
2. On the Kindle, open **Wireless Sync** from the Library. Tap **Find
   libraries**, then **Allow** your library.
3. Send books from Calibre as usual.

To stop the app managing series, set `"series": false` in
`/var/local/kindle-wireless-sync/state/config.json` and restart the Kindle.
The series it created are dissolved.

### Read status in Calibre

The Kindle can send each book's read status, and when you last read it, to
your Calibre library. It's off until you turn it on for a library:

1. In Calibre, open *Preferences → Add your own columns* and add two
   columns. The lookup names must be exactly these; the headings can be
   anything:
   - `kindle_read`, of type *Yes/No*
   - `kindle_last_read`, of type *Date*

   Restart Calibre when it asks.
2. On the Kindle, in kterm or over SSH, let the Kindle change that library.
   `status` lists your allowed libraries with their IDs:

   ```sh
   cd /var/local/kindle-wireless-sync
   ./kindle-wireless-sync status --state state
   ./kindle-wireless-sync allow --state state LIBRARY_ID --writable
   ```

From the next connection, books you mark read or unread on the Kindle, or
finish there, update `#kindle_read`, and opening a book updates
`#kindle_last_read`. Changes you make in Calibre stay in Calibre: they don't
change the Kindle. `--read-only` turns it off again.

## Development

The code is Rust, built for `armv7-unknown-linux-musleabihf` as one static
binary, with [`just`](https://github.com/casey/just) recipes for building,
testing and deploying to a Kindle over SSH. Start with:

- [`CLAUDE.md`](CLAUDE.md): the commands, the code layout and how to work
  with the device.
- [`docs/PLAN.md`](docs/PLAN.md): the phases, what's next, and deferred
  work.
- [`docs/decisions/`](docs/decisions/): the design decisions (ADRs).
- [`docs/research/`](docs/research/): notes on Calibre's protocol and the
  Kindle's internals, including on-device probes of firmware 5.20.1.

Changes are listed in [`CHANGELOG.md`](CHANGELOG.md).

## Credits

- [Calibre](https://github.com/kovidgoyal/calibre)'s smart device driver
  defines the protocol, and the
  [KOReader Calibre plugin](https://github.com/koreader/koreader/tree/master/plugins/calibre.koplugin)
  was a reference client implementation of it.
- [Kindle Button Mapper](https://github.com/zampierilucas/kindle-button-mapper-rs)
  was the model for the on-device app, the KPM packaging and the
  development workflow.
- [Kindle HID Passthrough](https://github.com/zampierilucas/kindle-hid-passthrough)
  was the model for the user-controlled service. It and Button Mapper
  supplied the crash-loop guard, a boot counter that turns autostart off.
- [kindle-usbnetlite](https://github.com/notmarek/kindle-usbnetlite)
  supplied the autostart switch: a sentinel file checked by an
  always-installed job, so turning autostart on or off never touches the
  read-only system partition.
- [LibrarianSync](https://github.com/barsanuphe/librariansync) and
  [kindle-series-manager](https://github.com/mlapaglia/kindle-series-manager)
  showed how to talk to the Kindle's catalog and how series are stored.

## Licence

Copyright (C) 2026 predakanga

This program is free software: you can redistribute it and/or modify it
under the terms of the GNU General Public License as published by the Free
Software Foundation, either version 3 of the License, or (at your option)
any later version.

This program is distributed in the hope that it will be useful, but WITHOUT
ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or
FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for
more details.

You should have received a copy of the GNU General Public License along with
this program (see [`LICENSE`](LICENSE)). If not, see
<https://www.gnu.org/licenses/>.

Each source file carries an `SPDX-License-Identifier: GPL-3.0-or-later`
header.
