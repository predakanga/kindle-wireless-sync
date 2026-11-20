// SPDX-License-Identifier: GPL-3.0-or-later
//! Messages the service shows the user, with a code the app translates
//! (ADR 0028). The English here goes to the log and the command line, and
//! is the app's fallback; `package/ui/strings-en.js` must have the same
//! template under the same code (checked by a test).

use std::fmt;

use serde_json::{Map, Value};

/// Every message: its code and English template. `{name}` is filled from
/// the message's parameter of that name.
const ENGLISH: &[(&str, &str)] = &[
    // Status (`detail`), as the command line and log show it.
    ("svc.paused", "Sync paused. Resume to reconnect."),
    ("svc.waiting_for_wifi", "Waiting for Wi-Fi."),
    ("svc.disconnecting", "Disconnecting once Calibre has finished what it's doing."),
    ("svc.not_linked", "No Calibre library is linked yet. Use Find libraries, or link one by address."),
    ("svc.looking", "Looking for {name}."),
    ("svc.connected", "Connected to {name}."),
    ("svc.ejected", "Ejected by Calibre. Will reconnect when Wi-Fi reconnects, or with Connect now."),
    ("svc.checking_still_open", "Disconnected by Calibre. Checking whether Calibre is still open."),
    ("svc.waiting_for_check", "Waiting while Calibre at the address of {name} is checked."),
    ("svc.calibre_closed_retrying", "Calibre has closed. Looking again in {secs}s."),
    ("svc.not_found_retrying", "Calibre library {name} not found. Retrying in {secs}s."),
    ("svc.busy_retrying", "Calibre is busy with another device. Retrying in {secs}s."),
    ("svc.lost_retrying", "Lost the connection to {name}. Retrying in {secs}s."),
    ("svc.locked_out_retrying", "Calibre may have stopped accepting this Kindle after too many failed attempts. In Calibre, stop and start the wireless device connection (Connect/share), then try again. Retrying in {secs}s."),
    ("svc.password_rejected", "Calibre library {name} rejected the password. Fix it, then use Connect now."),
    ("svc.cannot_reset", "Cannot reset for the newly linked library: {error}"),
    ("svc.stopped", "Stopped."),
    // Advice: Home's line under the status.
    ("svc.ejected_advice", "Ejected by Calibre.\nTap Connect below to reconnect."),
    ("svc.calibre_closed", "Can't connect to Calibre.\nTap Connect below to retry immediately."),
    ("svc.serving_other", "Calibre at {addr} is serving {other}, not {name}. Switch back to {name} in Calibre, then use Connect."),
    ("svc.lost", "Lost the connection to {name}."),
    // Advice, and a library check's result (ADR 0020).
    ("svc.busy", "Calibre is busy with another device."),
    ("svc.locked_out", "Calibre may have stopped accepting this Kindle after too many failed attempts. In Calibre, stop and start the wireless device connection (Connect/share), then try again."),
    ("svc.host_not_found", "Couldn't find {host} on the network."),
    ("svc.nothing_answered", "Nothing answered at {host}:{port}. Check that Calibre's wireless device connection is started."),
    ("svc.cannot_connect", "Couldn't connect to {host}:{port}: {error}"),
    ("svc.unsupported_protocol", "This version of Calibre's wireless connection isn't supported."),
    ("svc.ended_early", "Calibre ended the connection before the check finished."),
    // API errors.
    ("svc.cannot_read_settings", "Cannot read settings: {error}"),
    ("svc.cannot_save_settings", "Cannot save settings: {error}"),
    ("svc.cannot_change_autostart", "Cannot change autostart: {error}"),
    ("svc.folder_invalid", "{path}: {error}"),
    ("svc.folder_outside", "{path} leads outside the books folder"),
    ("svc.folder_not_folder", "{path} isn't a folder"),
    ("svc.folder_name_missing", "Enter a name for the folder."),
    ("svc.folder_name_slash", "A folder name can't contain \"/\"."),
    ("svc.folder_name_invalid", "{name}: {error}"),
    ("svc.folder_exists", "There's already a folder called {name}."),
    ("svc.cannot_create_folder", "Cannot create the folder: {error}"),
    ("svc.library_not_linked", "That library isn't linked."),
    ("svc.no_library_linked", "No library is linked."),
    ("svc.no_longer_linked", "That library is no longer linked."),
    ("svc.not_found_find_again", "That library isn't in the found list. Use Find libraries again."),
    ("svc.not_found_search_again", "That library isn't in the found list. Search again."),
    ("svc.enter_host", "Enter the computer's name or address."),
    ("svc.enter_port", "Enter a port between 1 and 65535."),
    ("svc.cannot_reach", "Could not reach Calibre at {host}:{port}: {error}"),
    ("svc.needs_replace", "This Kindle is linked to {linked}. Linking {library} replaces it."),
    ("svc.other_library_answered", "A different library answered: {library}."),
    ("svc.not_a_service", "Not running as a service."),
    ("svc.not_bool_column", "{column} isn't a Yes/No column in this library."),
    ("svc.not_date_column", "{column} isn't a Date column in this library."),
    ("svc.read_sync_unavailable", "Reading status sync needs Kindle firmware {min} or later. This Kindle has {firmware}."),
    ("svc.check_first", "Check the library first."),
];

/// A message for the user: a code from [`ENGLISH`] and its parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    code: &'static str,
    params: Vec<(&'static str, String)>,
}

impl Message {
    pub fn new(code: &'static str) -> Message {
        debug_assert!(template(code).is_some(), "no English for {code}");
        Message {
            code,
            params: Vec::new(),
        }
    }

    /// Adds the parameter `name`.
    pub fn with(mut self, name: &'static str, value: impl fmt::Display) -> Message {
        self.params.push((name, value.to_string()));
        self
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    /// The English text, for the log, the command line and `error` fields.
    pub fn english(&self) -> String {
        let Some(template) = template(self.code) else {
            return self.code.to_owned();
        };
        fill(template, &self.params)
    }

    /// For the app: `{"code": …, <name>: <value>, …}`.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("code".into(), self.code.into());
        for (name, value) in &self.params {
            out.insert((*name).into(), value.as_str().into());
        }
        Value::Object(out)
    }
}

impl std::error::Error for Message {}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.english())
    }
}

fn template(code: &str) -> Option<&'static str> {
    ENGLISH.iter().find(|(c, _)| *c == code).map(|(_, t)| *t)
}

/// Replaces each `{name}` with its parameter; unknown names stay as they
/// are. One pass, so a value containing braces isn't filled in again.
fn fill(template: &str, params: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let value = after.find('}').and_then(|end| {
            let name = &after[..end];
            params
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| (v, end))
        });
        match value {
            Some((v, end)) => {
                out.push_str(v);
                rest = &after[end + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    #[test]
    fn fills_parameters_once() {
        let m = Message::new("svc.serving_other")
            .with("addr", "10.0.0.2")
            .with("other", "{name}")
            .with("name", "Books");
        assert_eq!(
            m.english(),
            "Calibre at 10.0.0.2 is serving {name}, not Books. Switch back to Books in Calibre, then use Connect."
        );
        assert_eq!(
            m.to_json(),
            json!({"code": "svc.serving_other", "addr": "10.0.0.2", "other": "{name}", "name": "Books"})
        );
        assert_eq!(fill("{a} {b", &[("a", "x".into())]), "x {b");
    }

    #[test]
    fn codes_are_unique() {
        let codes: BTreeSet<_> = ENGLISH.iter().map(|(c, _)| c).collect();
        assert_eq!(codes.len(), ENGLISH.len());
    }

    /// The `"svc.…": "…"` entries of a strings file, unescaped.
    fn js_strings(text: &str) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix('"') else {
                continue;
            };
            let Some((key, value)) = rest.split_once("\": \"") else {
                continue;
            };
            let value = value.trim_end_matches(',');
            let Some(value) = value.strip_suffix('"') else {
                continue;
            };
            let mut s = String::new();
            let mut chars = value.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    match chars.next() {
                        Some('n') => s.push('\n'),
                        Some(other) => s.push(other),
                        None => {}
                    }
                } else {
                    s.push(c);
                }
            }
            out.insert(key.to_owned(), s);
        }
        out
    }

    #[test]
    fn the_apps_english_matches() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("package/ui/strings-en.js");
        let js = js_strings(&std::fs::read_to_string(path).unwrap());
        for (code, english) in ENGLISH {
            assert_eq!(
                js.get(*code).map(String::as_str),
                Some(*english),
                "strings-en.js differs for {code}"
            );
        }
        for key in js.keys().filter(|k| k.starts_with("svc.")) {
            assert!(
                template(key).is_some(),
                "strings-en.js has {key}, the service doesn't"
            );
        }
    }

    /// Every `"area.name"` string in script.js is a key in strings-en.js,
    /// and every key but the service's and the states' is used there.
    #[test]
    fn the_apps_keys_exist() {
        let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("package/ui");
        let en = js_strings(&std::fs::read_to_string(ui.join("strings-en.js")).unwrap());
        let script = std::fs::read_to_string(ui.join("script.js")).unwrap();
        let is_key = |s: &str| {
            s.split_once('.').is_some_and(|(a, b)| {
                [a, b]
                    .iter()
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
            })
        };
        // Every text between two double quotes, as the file mixes quotes.
        let used: BTreeSet<&str> = script.split('"').filter(|s| is_key(s)).collect();
        for key in &used {
            assert!(
                en.contains_key(*key),
                "script.js uses {key}, strings-en.js lacks it"
            );
        }
        for key in en.keys() {
            if !key.starts_with("svc.") && !key.starts_with("state.") {
                assert!(
                    used.contains(key.as_str()),
                    "strings-en.js has {key}, script.js never uses it"
                );
            }
        }
    }

    #[test]
    fn every_code_is_used_and_every_use_has_a_code() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut used = BTreeSet::new();
        for entry in std::fs::read_dir(&src).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|n| n == "message.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for part in text.split("Message::new(\"").skip(1) {
                used.insert(part.split('"').next().unwrap().to_owned());
            }
        }
        for code in &used {
            assert!(template(code).is_some(), "{code} has no English");
        }
        for (code, _) in ENGLISH {
            assert!(used.contains(*code), "{code} is never used");
        }
    }
}
