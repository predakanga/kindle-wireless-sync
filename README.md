# Kindle Wireless Sync

[![CI](https://github.com/predakanga/kindle-wireless-sync/actions/workflows/ci.yml/badge.svg)](https://github.com/predakanga/kindle-wireless-sync/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/predakanga/kindle-wireless-sync)](https://github.com/predakanga/kindle-wireless-sync/releases/latest)
[![Licence: GPL-3.0-or-later](https://img.shields.io/badge/licence-GPL--3.0--or--later-blue)](LICENSE)

Sends books from Calibre to a jailbroken Kindle over Wi-Fi. They go straight
into the Kindle's own library, with no USB cable, no extra software on your
computer, and no reboot or rescan.

It uses Calibre's built-in **Wireless device connection**, so your computer
needs nothing but stock Calibre. The books land in the Kindle's built-in
reader. KOReader and other third-party readers are out of scope.

> [!WARNING]
> **Written with AI**
>
> - This project was written using AI (Anthropic's Claude, through Claude
>   Code), directed and reviewed by its developer.
>
> It modifies a jailbroken device. Use it at your own risk, and keep your
> books backed up in Calibre. It comes with no warranty (see
> [Licence](#licence)).

## Contents

- [Features](#features)
- [Requirements](#requirements)
- [Compatibility](#compatibility)
- [Install](#install)
- [Use](#use)
  - [Read status in Calibre](#read-status-in-calibre)
- [Development](#development)
- [Credits](#credits)
- [Licence](#licence)

## Features

- **Wireless sending from Calibre.** Calibre sees the Kindle as a wireless
  device: send, replace and delete books as usual. Each book appears in the
  library as soon as its transfer finishes.
- **Calibre linking.** The Kindle only talks to the Calibre library you
  link it to, found on your network or added by address. Password-protected
  Calibre connections work too.
- **GUI configuration.** A **Wireless Sync** tile provides a UI for the app.
  It shows the status, allows you to link to a Calibre library, and
  configures the app's settings.
- **Native series.** Calibre series are converted to native Kindle series,
  grouping into a single tile and showing in series order. Note: Kindle
  series grouping is [only available in certain
  markets](https://github.com/predakanga/kindle-series-enabler).
- **Compatible with USB sync.** Books that Calibre has previously sent to
  the Kindle over USB are automatically adopted and shown in Calibre's
  device view.

Supported Formats: AZW3, MOBI, AZW, KFX, PDF and TXT. Other formats will be
converted by Calibre prior to sending.

## Requirements

- A jailbroken Kindle with [KPM](https://kindlemodding.org/jailbreaking/whats-next/installing-homebrew.html#kpm)
  (the Kindle Package Manager) installed.
- Calibre on a computer on the same network. A different network also works,
  using *Connect by address* in the app.

## Compatibility

Wireless Sync is only compatible with Kindles running **firmware 5.16.3 or
later** (requires the `kindlehf` platform).

The following table lists the devices the app has been tested on, and
whether they were real or
[emulated](https://github.com/katadelos/eink-emulator):

| Model | Firmware Version | Tested on |
|---|---|---|
| Kindle (10th Generation) | 5.18.1.1.1 | ⚠️[^1] Emulated |
| Kindle (11th Generation) | 5.19.2.0.1 | ✅ Emulated |
| Kindle (2024) | 5.20.1 | ✅ Emulated |
| Kindle Colorsoft | 5.20.1 | ✅ Emulated |
| Kindle Oasis (10th Generation) | 5.18.2.1.1 | ⚠️[^1] Emulated |
| Kindle Paperwhite (10th Generation) | 5.18.1.1.1 | ⚠️[^1] Emulated |
| Kindle Paperwhite (11th Generation) | 5.19.2 | ✅ Emulated |
| Kindle Paperwhite (12th Generation) | 5.20.1 | ✅ Real device |
| Kindle Scribe (2022) | 5.20.1 | ✅ Emulated |
| Kindle Scribe (2024) | 5.20.1 | ✅ Emulated |
| Kindle Scribe (2025) | 5.20.1 | ✅ Emulated |
| Kindle Scribe Colorsoft | 5.20.1 | ✅ Emulated |

[^1]: Latest firmware doesn't support read status sync

If you try Wireless Sync on another model or firmware, please [let us
know](https://github.com/predakanga/kindle-wireless-sync/issues) if it
worked or not.

## Install

With Wi-Fi on, [use
KPM](https://kindlemodding.org/jailbreaking/whats-next/installing-homebrew.html)
to add our repository and install the app:

```
;kpm add-repo https://predakanga.github.io/kindle-apps/repo.json
;kpm update
;kpm install kindle-wireless-sync
```

To upgrade, run `;kpm update` and `;kpm install kindle-wireless-sync` again.
Your settings are kept.

To uninstall, run `;kpm uninstall kindle-wireless-sync`. Books you have
received will stay in your library, but the series this app created are
removed.

## Use

1. In Calibre, choose *Connect/share → Start wireless device connection*.
2. On the Kindle, open **Wireless Sync** from the Library. It lists the
   Calibre libraries it finds; tap yours. The app tests the connection,
   asking for Calibre's password if it has one, then shows the library's
   settings. Tap **Link** to confirm.
3. Send books from Calibre as usual.

To use a different Calibre library, tap **Change library** on the app's
first page and choose it in the same way. Your synced books stay on the
Kindle, including their series, until the new library sends its own. Books
the new library doesn't have show in Calibre as on the device only.

To stop the app managing series, turn off **Group books into series** in
the app's settings (the gear). The series it created are dissolved.

New books go into the Kindle's `documents` folder by default. To change
this, open the app's settings (the gear), tap **Change** next to **Storage
folder**, and pick or create one. Only new books will be placed there: books
already on the Kindle stay where they are.

By default, the app does not run until you first open the UI. To change
this, open the app's settings (the gear) and enable autostart.

### Read status in Calibre

The Kindle can send each book's read status, and when you last read it, to
your Calibre library. It needs Kindle firmware 5.19.1 or later (see
[Compatibility](#compatibility)). It's off until you turn it on for a
library:

1. In Calibre, open *Preferences → Add your own columns* and add a column
   of type *Yes/No* (for example "Kindle read") and one of type *Date*
   (for example "Kindle last read"). Restart Calibre when it asks, and
   start its wireless device connection again.
2. In the app, turn on **Update read status in Calibre**, when linking the
   library or later under **Library settings**, and choose the two
   columns. Columns with the lookup names `kindle_read` and
   `kindle_last_read` are picked for you.

From the next connection, books you mark read or unread on the Kindle, or
finish there, update the Yes/No column, and opening a book updates the Date
column. Changes you make in Calibre stay in Calibre: they don't change the
Kindle. Linking a different library turns it off.

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
- [eink-emulator](https://github.com/katadelos/eink-emulator) let us test
  the app on lots of different models and versions of Kindle.

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
