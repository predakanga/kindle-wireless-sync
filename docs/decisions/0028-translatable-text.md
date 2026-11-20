# 0028: Translatable text

- Status: Proposed
- Date: 2026-11-20

## Context
Later 2b: move the app's text into a file, so it can be translated
(developer, split from the UI review on 2026-10-21). No translation is
needed yet (developer, 2026-11-20), only the means to add one.

**Where the app's text comes from.**
- `package/ui/script.js` holds most of it (pages, buttons, hints), and
  builds the status line from the service's state codes.
- About 35 messages are made by the service, in English, and shown as
  they are: the status's `advice`, its `detail` in the `error` state, API
  errors (`{"ok": false, "error": …}`), and the library check's `error`
  (ADR 0020). Some carry a system error (`{e}`), which has no translation.
- The service's English also goes to its log and to the CLI (`status`
  prints `status.json`; `find`, `link` and `unlink` print messages).

**The Kindle's language** is in `/var/local/system/locale`
(`LANG=en_GB.utf8` on the reference device, 5.20.1). Whether mesquite's
`navigator.language` follows it is unchecked.

**Packaging assumes a flat `ui/`:** `install.sh` and `tools/package.sh`
copy and `chmod 644` everything in it, which would make a subfolder
unreadable to the framework user (ADR 0012).

## Decision
1. **A strings file per language,** `package/ui/strings-<lang>.js`, plain
   ES5 that adds a table to `window.KWS_STRINGS`:
   `KWS_STRINGS.en = {"home.title": "Wireless Sync", …}`. Keys are names,
   not English text. Placeholders are `{name}`. Templates may hold simple
   HTML (`<b>`); values filled in are escaped. English (`strings-en.js`)
   is the source and the fallback for any missing key.
2. **The app picks the language from the service:** `/status` gains
   `locale` (`en_GB`), read from `/var/local/system/locale`, or `LANG`
   off a Kindle. The app loads `strings-en.js` up front, then
   `strings-en_GB.js` or else `strings-en.js`'s language part (`strings-de.js`
   for `de_DE`) by adding a `<script>`; a missing file leaves English.
3. **The service sends a code alongside its English.** Each message is a
   `message::Message`: a code, named parameters, and an English template
   in one table in `src/message.rs`. JSON carries both:
   `"advice": "Calibre is busy …", "advice_msg": {"code": "busy"}`, and
   likewise `detail_msg`, and `message` beside an API or check `error`.
   The app shows the code's translation when it has one, otherwise the
   English. The log and the CLI keep the English.
4. **A test keeps the English in step:** every code in `src/message.rs`
   must have the same template under the same key in `strings-en.js`.

## Alternatives considered
- **JSON strings files fetched with XHR:** unproven from `file://` in
  mesquite, and an extra failure path before anything can be drawn.
- **gettext `.po` files:** the usual translator format, but they need a
  build step to become something ES5 can load. Can be added later by
  generating the JS file from them.
- **The service translates its own messages:** two sets of translation
  files, and the binary carries every language.
- **Only `script.js`'s strings now:** the service's messages are the ones
  users read when something is wrong, so they would stay English exactly
  when it matters.
- **English text as keys:** fewer names to invent, but fixing a typo in
  English then breaks every translation.

## Consequences
- The English of a service message lives in two places, kept identical by
  the test.
- A new message needs a code, a template in `src/message.rs`, and the same
  template in `strings-en.js`.
- An app older than the service ignores the new fields; a newer app with
  an older service falls back to the English.
- System errors (`{e}`) and Calibre's library names stay as they are.
- The library tile's name and the KPM description stay English.

## Implementation notes (2026-11-20)
- **Folder errors** come from `folders::resolve`, `list` and `create` as
  messages now, and replace the API's "Cannot open that folder: …" and
  "Cannot use that folder: …" wrappers, so the app shows only the reason
  ("Calibre/x leads outside the books folder"). A path or system error
  inside one (`svc.folder_invalid`) stays English.
- **Find's per-server errors** carry a `message` when they're Calibre's
  doing (busy, the lockout); other network errors stay English.
- **Checked** on the Mac in headless Chrome with a partial dummy
  translation, and on the reference device (5.20.1, `en_GB`): mesquite
  loaded a temporary `strings-en_GB.js` added by `<script>`, and with no
  such file the app is unchanged.
