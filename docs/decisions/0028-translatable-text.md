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
1. **A JSON file per language,** `package/i18n/<lang>.json`: one object of
   keys and text (`{"home.status": "Status", …}`), the plain key/value
   format translation platforms such as Weblate take (developer,
   2026-11-20). Keys are names, not English text. Placeholders are
   `{name}`. Templates may hold simple HTML (`<b>`); values filled in are
   escaped. English (`en.json`) is the source and the fallback for any
   missing key. Notes for translators are in `package/i18n/TRANSLATING.md`.
2. **Built into scripts:** `tools/strings.py` (`just strings`, also run by
   `just deploy-ui` and `tools/package.sh`) turns each into
   `package/ui/strings-<lang>.js`, plain ES5 that adds its table to
   `window.KWS_STRINGS`. They're build output, ignored by git, so a
   platform's commits (JSON only) never leave them stale.
3. **The app picks the language from the service:** `/status` gains
   `locale` (`en_GB`), read from `/var/local/system/locale`, or `LANG`
   off a Kindle. `index.html` loads `strings-en.js` with a `<script>` tag,
   so English is there before the first page is drawn. The app then adds
   `<script>` tags for `strings-<locale>.js` and its language alone
   (`strings-de.js` for `de_DE`); the most specific one that loads wins,
   and a missing file leaves English.
4. **The service sends a code alongside its English.** Each message is a
   `message::Message`: a code, named parameters, and an English template
   in one table in `src/message.rs`. JSON carries both:
   `"advice": "Calibre is busy …", "advice_msg": {"code": "busy"}`, and
   likewise `detail_msg`, and `message` beside an API or check `error`.
   The app shows the code's translation when it has one, otherwise the
   English. The log and the CLI keep the English.
5. **Tests keep the English in step:** every code in `src/message.rs` must
   have the same template under the same key in `en.json`, and every key
   `script.js` uses must be in `en.json`, and the other way round.

## Alternatives considered
- **Strings files written as JavaScript** (`strings-en.js` as the source,
  the first version of this decision): no build step, but no translation
  platform reads that format.
- **Loading the JSON at runtime with XHR.** Mesquite allows it (reference
  device, 2026-11-20: a JSON file next to `index.html` loaded and parsed;
  a missing one gives status 0 and empty text, no exception; `fetch`
  doesn't exist). But desktop Chrome and Firefox block XHR between
  `file://` pages, so opening the app in a browser on the Mac, as the
  development set-up does (ADR 0011), would show no text. Generating
  English as a script but translations as JSON would mean two ways of
  loading; generating every language is one.
- **gettext `.po` files:** the usual translator format, and Weblate takes
  them, but key/value JSON is simpler to build from and to check in a test.
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
  template in `en.json`.
- Opening `package/ui/index.html` by hand needs `just strings` first.
  Building needs Python 3 (already used by the tools; on CI's Ubuntu).
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
  such file the app is unchanged. Moved to JSON sources the same day,
  after the developer asked about Weblate.
