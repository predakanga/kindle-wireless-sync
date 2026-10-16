# Kindle Wireless Sync

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
  was the model for the user-controlled service and its autostart.
- [LibrarianSync](https://github.com/barsanuphe/librariansync) and
  [kindle-series-manager](https://github.com/mlapaglia/kindle-series-manager)
  showed how to talk to the Kindle's catalog and how series are stored.

## Licence

GNU General Public License v3.0 or later. See [`LICENSE`](LICENSE).
