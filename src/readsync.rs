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
    /// `custom` maps lookup names to Calibre datatypes.
    pub fn available_in(&self, custom: &Map<String, Value>) -> Columns {
        let has = |name: &Option<String>, datatype: &str| {
            name.clone()
                .filter(|n| custom.get(n).and_then(Value::as_str) == Some(datatype))
        };
        Columns {
            read: has(&self.read, "bool"),
            last_read: has(&self.last_read, "datetime"),
        }
    }
}

/// A library's custom columns and their datatypes, from the `fieldMetadata`
/// of Calibre's `SET_LIBRARY_INFO`.
pub fn custom_columns(field_metadata: &Value) -> Map<String, Value> {
    field_metadata
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, fm)| name.starts_with('#') && fm["is_custom"] == json!(true))
        .filter_map(|(name, fm)| Some((name.clone(), json!(fm["datatype"].as_str()?))))
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
    let dir = books_dir.to_string_lossy().replace('\'', "''");
    let rows = catalog.query(&format!(
        "SELECT p_location, p_readState, p_lastOpenTime FROM Entries \
         WHERE p_type = 'Entry:Item' AND p_location LIKE '{dir}/%'"
    ))?;
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
        let kindle = kindle.and_then(|k| k.last_open).map(|t| json!(t));
        let value = pick(&mut state, "last_read", kindle, calibre);
        // We track the Kindle's time as Unix seconds; Calibre wants ISO.
        let value = match value.as_i64() {
            Some(t) => json!(iso_utc(UNIX_EPOCH + Duration::from_secs(t.max(0) as u64))),
            None => value,
        };
        out.insert("_last_read_date_".into(), value);
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

/// The Kindle's value if it changed since we last reported it (recording it
/// as reported), otherwise Calibre's own value.
fn pick(state: &mut Value, key: &str, kindle: Option<Value>, calibre: Option<Value>) -> Value {
    match kindle {
        Some(k) if state.get(key) != Some(&k) => {
            state[key] = k.clone();
            k
        }
        _ => calibre.unwrap_or(Value::Null),
    }
}

/// The value Calibre last sent for a column, or `None` if the book's
/// metadata doesn't have the column at all.
fn calibre_value(record: &Value, column: &str) -> Option<Value> {
    let meta = record.get("user_metadata")?.get(column)?;
    Some(meta.get("#value#").cloned().unwrap_or(Value::Null))
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
            "#kindle_read": {"is_custom": true, "datatype": "bool"},
            "#kindle_last_read": {"is_custom": true, "datatype": "text"},
            "title": {"is_custom": false, "datatype": "text"},
        });
        let custom = custom_columns(&fm);
        assert_eq!(custom.len(), 2);
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

    #[test]
    fn reports_a_kindle_change_once_then_echoes_calibre() {
        let named = Columns::defaults();
        let mut rec = record(Some(json!(false)), Some(Value::Null));
        let k = kindle(1, Some(86_400));

        let out = report(&mut rec, &named, Some(&k));
        assert_eq!(out["_is_read_"], json!(true));
        assert_eq!(out["_last_read_date_"], json!("1970-01-02T00:00:00+00:00"));
        assert!(out.get("_sync_type_").is_none());

        // Unchanged on the Kindle: Calibre's value comes back, even though
        // Calibre (say the user unticked it) now differs.
        let out = report(&mut rec, &named, Some(&k));
        assert_eq!(out["_is_read_"], json!(false));
        assert_eq!(out["_last_read_date_"], Value::Null);

        // A new change on the Kindle is reported again.
        let out = report(&mut rec, &named, Some(&kindle(3, Some(90_000))));
        assert_eq!(out["_is_read_"], json!(false));
        assert_eq!(out["_last_read_date_"], json!("1970-01-02T01:00:00+00:00"));
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
