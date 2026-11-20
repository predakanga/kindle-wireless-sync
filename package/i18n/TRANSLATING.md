<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Translating Wireless Sync

The app's text is in this folder, one JSON file per language. `en.json` is
English: the source, and what the app shows for anything a translation
leaves out (ADR 0028).

To add a language, copy `en.json` to `<language>.json` (`de.json`, or
`pt_BR.json` for one country) and translate the text on the right of each
line. The Kindle's language picks the file: a Kindle set to `de_DE` uses
`de_DE.json` if there is one, otherwise `de.json`.

- Keep each `{name}` exactly as it is: the app fills it in.
- `<b>` and `<i>` may move, but keep them.
- `\n` is a line break.
- Keys starting `svc.` are messages from the service. Their English is also
  in `src/message.rs`; a test keeps the two the same, so change both when
  changing the English.

`just strings` turns these files into the app's `package/ui/strings-<language>.js`
(`tools/strings.py`); packaging and `just deploy-ui` do it too. Those files
aren't kept in git.
