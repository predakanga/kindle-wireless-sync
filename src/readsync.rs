// SPDX-License-Identifier: GPL-3.0-or-later
//! Read status and last-read date, from the Kindle to Calibre (ADR 0017).
//!
//! Calibre's wireless driver syncs two custom columns the device names in its
//! handshake: a Yes/No column (`_is_read_`) and a Date column
//! (`_last_read_date_`). For each book, it takes a value that differs from
//! the one it last sent the device as a change made on the device. So we
//! report the Kindle's value only when it changed since we last reported it,
//! and otherwise give Calibre its own value back.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::adopt::iso_utc;
use crate::catalog::Catalog;

/// The lookup names the app stores for now (ADR 0017).
pub const DEFAULT_READ_COLUMN: &str = "#kindle_read";
pub const DEFAULT_LAST_READ_COLUMN: &str = "#kindle_last_read";

/// Our per-book state, inside the record's private `_kws` object.
const STATE_KEY: &str = "read_sync";

/// The two sync columns, by lookup name. Either may be absent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Columns {
    pub read: Option<String>,
    pub last_read: Option<String>,
}

impl Columns {
    pub fn defaults() -> Columns {
        Columns {
            read: Some(DEFAULT_READ_COLUMN.into()),
            last_read: Some(DEFAULT_LAST_READ_COLUMN.into()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.read.is_none() && self.last_read.is_none()
    }

    /// The ones that exist in a library, with the type Calibre requires.
    /// `custom` is a library's columns, as [`custom_columns`] lists them.
    pub fn available_in(&self, custom: &Map<String, Value>) -> Columns {
        let has = |name: &Option<String>, datatype: &str| {
            name.clone()
                .filter(|n| custom.get(n).and_then(|c| c["datatype"].as_str()) == Some(datatype))
        };
        Columns {
            read: has(&self.read, "bool"),
            last_read: has(&self.last_read, "datetime"),
        }
    }
}

/// A library's custom columns, from the `fieldMetadata` of Calibre's
/// `SET_LIBRARY_INFO`: lookup name to `{"name": heading, "datatype": type}`
/// (ADR 0020). The heading falls back to the lookup name.
pub fn custom_columns(field_metadata: &Value) -> Map<String, Value> {
    field_metadata
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(lookup, fm)| lookup.starts_with('#') && fm["is_custom"] == json!(true))
        .filter_map(|(lookup, fm)| {
            let datatype = fm["datatype"].as_str()?;
            let name = fm["name"].as_str().unwrap_or(lookup);
            Some((lookup.clone(), json!({"name": name, "datatype": datatype})))
        })
        .collect()
}

/// What the Kindle holds about one book.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct KindleState {
    /// `p_readState`: 0 = never set, 1 = marked read, 2 = marked read
    /// automatically at the end, 3 = marked unread.
    pub read_state: i64,
    /// `p_lastOpenTime`, Unix seconds; unset until the book is first opened.
    pub last_open: Option<i64>,
}

impl KindleState {
    pub fn is_read(&self) -> Option<bool> {
        match self.read_state {
            1 | 2 => Some(true),
            3 => Some(false),
            _ => None,
        }
    }
}

/// The Kindle's state for every book under `books_dir`, by file path.
/// Read-only; fails if the catalog is busy.
pub fn kindle_states(
    catalog: &Catalog,
    books_dir: &Path,
) -> io::Result<HashMap<String, KindleState>> {
    let rows = catalog.query(
        "SELECT p_location, p_readState, p_lastOpenTime FROM Entries \
         WHERE p_type = 'Entry:Item' AND p_location LIKE ?1",
        &[&format!("{}/%", books_dir.to_string_lossy())],
    )?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let location = r.get("p_location")?.as_str()?.to_owned();
            let state = KindleState {
                read_state: r.get("p_readState").and_then(Value::as_i64).unwrap_or(0),
                last_open: r
                    .get("p_lastOpenTime")
                    .and_then(Value::as_i64)
                    .filter(|t| *t > 0),
            };
            Some((location, state))
        })
        .collect())
}

/// The fields to add to one book's entry in the book list, given the columns
/// named in this session. Records in `record` what we reported, so the
/// caller must save the records afterwards.
pub fn report(
    record: &mut Value,
    named: &Columns,
    kindle: Option<&KindleState>,
) -> Map<String, Value> {
    let mut out = Map::new();
    if named.is_empty() || !record.is_object() {
        return out;
    }
    let mut state = record["_kws"][STATE_KEY].clone();
    if !state.is_object() {
        state = json!({});
    }
    // A book whose metadata predates the column: we don't know Calibre's
    // value, so let Calibre treat this as a first sync (`_sync_type_` 2),
    // where the device's value only wins if it has one.
    let mut calibre_unknown = false;

    if let Some(col) = &named.read {
        let calibre = calibre_value(record, col);
        calibre_unknown |= calibre.is_none();
        let kindle = kindle.and_then(KindleState::is_read).map(Value::Bool);
        out.insert(
            "_is_read_".into(),
            pick(&mut state, "read", kindle, calibre),
        );
    }
    if let Some(col) = &named.last_read {
        let calibre = calibre_value(record, col);
        calibre_unknown |= calibre.is_none();
        // As Calibre writes dates, so the two compare as strings.
        let kindle = kindle
            .and_then(|k| k.last_open)
            .map(|t| json!(iso_utc(UNIX_EPOCH + Duration::from_secs(t.max(0) as u64))));
        out.insert(
            "_last_read_date_".into(),
            pick(&mut state, "last_read", kindle, calibre),
        );
    }
    if calibre_unknown {
        out.insert("_sync_type_".into(), json!(2));
    }
    if !record["_kws"].is_object() {
        record["_kws"] = json!({});
    }
    record["_kws"][STATE_KEY] = state;
    out
}

/// Drops a book's read-sync state from its private `_kws` object, when the
/// Kindle is linked to a different library (ADR 0018).
pub fn forget(private: &mut Value) {
    if let Some(obj) = private.as_object_mut() {
        obj.remove(STATE_KEY);
    }
}

/// The Kindle's value if it changed since we last reported it, or if Calibre
/// hasn't taken it yet; otherwise Calibre's own value.
///
/// Calibre can skip a book's sync without telling us (it swallows errors),
/// so a report only counts once Calibre's metadata for the book shows the
/// value (`<key>_ok`). Calibre resends a book's metadata after taking a
/// value, so that's normally the next connection.
fn pick(state: &mut Value, key: &str, kindle: Option<Value>, calibre: Option<Value>) -> Value {
    let ok_key = format!("{key}_ok");
    let Some(k) = kindle else {
        return calibre.unwrap_or(Value::Null);
    };
    if state.get(key) != Some(&k) {
        // A new change on the Kindle.
        state[key] = k.clone();
        state[ok_key.as_str()] = json!(false);
        return k;
    }
    if calibre.as_ref() == Some(&k) {
        state[ok_key.as_str()] = json!(true);
    }
    if state[ok_key.as_str()] == json!(true) {
        calibre.unwrap_or(Value::Null)
    } else {
        // Reported, but Calibre doesn't have it: report it again.
        k
    }
}

/// The value Calibre last sent for a column, or `None` if the book's
/// metadata doesn't have the column at all. Calibre writes an empty date as
/// the string "None"; that comes back as null, because Calibre fails to
/// parse "None" as a date and then skips the book's sync altogether.
fn calibre_value(record: &Value, column: &str) -> Option<Value> {
    let meta = record.get("user_metadata")?.get(column)?;
    Some(match meta.get("#value#") {
        Some(Value::String(s)) if s == "None" => Value::Null,
        Some(v) => v.clone(),
        None => Value::Null,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(read: Option<Value>, last: Option<Value>) -> Value {
        let mut um = Map::new();
        if let Some(v) = read {
            um.insert(
                DEFAULT_READ_COLUMN.into(),
                json!({"datatype": "bool", "#value#": v}),
            );
        }
        if let Some(v) = last {
            um.insert(
                DEFAULT_LAST_READ_COLUMN.into(),
                json!({"datatype": "datetime", "#value#": v}),
            );
        }
        json!({"lpath": "a.azw3", "user_metadata": um, "_kws": {"cde_key": "k"}})
    }

    fn kindle(read_state: i64, last_open: Option<i64>) -> KindleState {
        KindleState {
            read_state,
            last_open,
        }
    }

    #[test]
    fn picks_typed_custom_columns() {
        let fm = json!({
            "#kindle_read": {"is_custom": true, "datatype": "bool", "name": "Kindle read"},
            "#kindle_last_read": {"is_custom": true, "datatype": "text"},
            "title": {"is_custom": false, "datatype": "text"},
        });
        let custom = custom_columns(&fm);
        assert_eq!(custom.len(), 2);
        assert_eq!(
            custom["#kindle_read"],
            json!({"name": "Kindle read", "datatype": "bool"})
        );
        assert_eq!(custom["#kindle_last_read"]["name"], "#kindle_last_read");
        let avail = Columns::defaults().available_in(&custom);
        assert_eq!(avail.read.as_deref(), Some("#kindle_read"));
        assert_eq!(avail.last_read, None, "wrong type");
        assert!(Columns::defaults().available_in(&Map::new()).is_empty());
    }

    #[test]
    fn maps_the_kindle_read_state() {
        let r: Vec<_> = (0..=4).map(|s| kindle(s, None).is_read()).collect();
        assert_eq!(r, [None, Some(true), Some(true), Some(false), None]);
    }

    /// Calibre taking our values: it resends the book's metadata with them.
    fn calibre_takes(rec: &mut Value, out: &Map<String, Value>) {
        rec["user_metadata"][DEFAULT_READ_COLUMN]["#value#"] = out["_is_read_"].clone();
        rec["user_metadata"][DEFAULT_LAST_READ_COLUMN]["#value#"] = out["_last_read_date_"].clone();
    }

    #[test]
    fn reports_a_kindle_change_until_calibre_takes_it_then_echoes_calibre() {
        let named = Columns::defaults();
        let mut rec = record(Some(json!(false)), Some(json!("None")));
        let k = kindle(1, Some(86_400));

        let out = report(&mut rec, &named, Some(&k));
        assert_eq!(out["_is_read_"], json!(true));
        assert_eq!(out["_last_read_date_"], json!("1970-01-02T00:00:00+00:00"));
        assert!(out.get("_sync_type_").is_none());

        // Calibre skipped it (its metadata is unchanged): report it again.
        let again = report(&mut rec, &named, Some(&k));
        assert_eq!(again, out);

        // Once Calibre has it, Calibre's value comes back, even after the
        // user unticks it in Calibre.
        calibre_takes(&mut rec, &out);
        assert_eq!(report(&mut rec, &named, Some(&k)), out);
        rec["user_metadata"][DEFAULT_READ_COLUMN]["#value#"] = json!(false);
        let out = report(&mut rec, &named, Some(&k));
        assert_eq!(out["_is_read_"], json!(false));
        assert_eq!(out["_last_read_date_"], json!("1970-01-02T00:00:00+00:00"));

        // A new change on the Kindle is reported again.
        let out = report(&mut rec, &named, Some(&kindle(3, Some(90_000))));
        assert_eq!(out["_is_read_"], json!(false));
        assert_eq!(out["_last_read_date_"], json!("1970-01-02T01:00:00+00:00"));
    }

    #[test]
    fn calibre_empty_date_goes_back_as_null() {
        // Calibre can't parse "None" as a date, and then skips the book.
        let mut rec = record(Some(json!(true)), Some(json!("None")));
        let out = report(&mut rec, &Columns::defaults(), None);
        assert_eq!(out["_last_read_date_"], Value::Null);
        assert_eq!(out["_is_read_"], json!(true));
    }

    #[test]
    fn no_kindle_value_echoes_calibre() {
        let named = Columns::defaults();
        let mut rec = record(Some(json!(true)), Some(json!("2026-01-01T00:00:00+00:00")));
        for k in [None, Some(kindle(0, None))] {
            let out = report(&mut rec, &named, k.as_ref());
            assert_eq!(out["_is_read_"], json!(true));
            assert_eq!(out["_last_read_date_"], json!("2026-01-01T00:00:00+00:00"));
        }
    }

    #[test]
    fn unknown_calibre_value_is_a_first_sync() {
        let named = Columns::defaults();
        let mut rec = record(None, None);
        let out = report(&mut rec, &named, Some(&kindle(0, None)));
        assert_eq!(out["_sync_type_"], json!(2));
        assert_eq!(out["_is_read_"], Value::Null);
        let out = report(&mut rec, &named, Some(&kindle(2, None)));
        assert_eq!(out["_is_read_"], json!(true));
    }

    #[test]
    fn only_named_columns_are_reported() {
        let mut rec = record(Some(json!(true)), None);
        let named = Columns {
            read: Some(DEFAULT_READ_COLUMN.into()),
            last_read: None,
        };
        let out = report(&mut rec, &named, Some(&kindle(3, Some(5))));
        assert_eq!(out.len(), 1);
        assert!(report(&mut rec, &Columns::default(), None).is_empty());
    }
}
