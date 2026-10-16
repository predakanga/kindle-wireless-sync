// SPDX-License-Identifier: GPL-3.0-or-later
//! Calibre series as the Kindle's native series grouping (ADR 0016).
//!
//! Each pass reads the catalog (read-only), works out what our series
//! should look like from the books we track, and sends only the
//! ChangeRequest commands that differ. Our series are recognised by their
//! key (`KW` + 8 base-32 characters), so we never touch Amazon's. Order
//! matters: books are un-hidden before their series goes away, and hidden
//! only once their series exists.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::Path;

use serde_json::{json, Value};

use crate::catalog::Catalog;

const SERIES_ID_PREFIX: &str = "urn:collection:1:asin-";
const KEY_PREFIX: &str = "KW";
/// The Library hides series with fewer members than this.
const MIN_MEMBERS: usize = 2;

/// A book we track, as far as series are concerned.
#[derive(Debug, Clone, PartialEq)]
pub struct Book {
    pub cde_key: String,
    pub cde_type: String,
    pub series: String,
    pub index: f64,
    pub title: String,
    pub authors: Vec<String>,
}

/// The books in our records that are in a Calibre series and have a
/// content key.
pub fn books_from_records(records: &[Value]) -> Vec<Book> {
    records
        .iter()
        .filter_map(|r| {
            let series = r["series"].as_str()?.trim();
            if series.is_empty() {
                return None;
            }
            Some(Book {
                cde_key: r["_kws"]["cde_key"].as_str()?.to_owned(),
                cde_type: r["_kws"]["cde_type"].as_str()?.to_owned(),
                series: series.to_owned(),
                index: r["series_index"].as_f64().unwrap_or(0.0),
                title: r["title"].as_str().unwrap_or_default().to_owned(),
                authors: r["authors"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// The ASIN-like key for a series name: "KW" and 8 base-32 characters of
/// the SHA-1 of the trimmed, lower-cased name. Exactly 10 characters,
/// because the catalog takes the last 10 characters of a series id.
pub fn series_key(name: &str) -> String {
    let digest = sha1_smol::Sha1::from(name.trim().to_lowercase())
        .digest()
        .bytes();
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    // 8 characters × 5 bits = the first 40 bits of the digest.
    let mut bits = digest[..5]
        .iter()
        .fold(0u64, |acc, b| acc << 8 | u64::from(*b));
    let mut key = [0u8; 8];
    for c in key.iter_mut().rev() {
        *c = ALPHABET[(bits & 31) as usize];
        bits >>= 5;
    }
    format!("{KEY_PREFIX}{}", std::str::from_utf8(&key).unwrap())
}

fn is_our_key(key: &str) -> bool {
    key.len() == 10
        && key.starts_with(KEY_PREFIX)
        && key[2..]
            .bytes()
            .all(|b| b.is_ascii_uppercase() || (b'2'..=b'7').contains(&b))
}

fn series_id(key: &str) -> String {
    format!("{SERIES_ID_PREFIX}{key}")
}

/// A stable uuid for a series container, from its key (name-based, like a
/// version 5 uuid), so a container is the same across passes and devices.
fn container_uuid(key: &str) -> String {
    let d = sha1_smol::Sha1::from(format!("kindle-wireless-sync series {key}"))
        .digest()
        .bytes();
    let mut b = [0u8; 16];
    b.copy_from_slice(&d[..16]);
    b[6] = (b[6] & 0x0f) | 0x50;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// How Calibre shows a series index: "2", "2.5".
fn index_label(index: f64) -> String {
    if index.fract() == 0.0 {
        format!("{}", index as i64)
    } else {
        let s = format!("{index:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

/// A book's row in the catalog.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    pub uuid: String,
    pub thumbnail: String,
    pub series_state: Option<i64>,
    pub origin_type: Option<i64>,
}

/// One of our series containers in the catalog.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Container {
    pub uuid: String,
    pub title: String,
    pub member_count: i64,
    pub home_member_count: i64,
    pub is_processed: i64,
    pub thumbnail: String,
    pub virtual_count: i64,
    pub members: Vec<String>,
}

/// One of our `Series` rows in the catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub key: String,
    pub cde_key: String,
    pub position: f64,
    pub label: String,
}

/// What the catalog holds that matters to our series.
#[derive(Debug, Default)]
pub struct Snapshot {
    /// Books under the books directory, by (cdeKey, cdeType).
    pub entries: HashMap<(String, String), Entry>,
    /// Our containers, by key.
    pub containers: HashMap<String, Container>,
    pub rows: Vec<Row>,
}

fn int(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

/// Reads the snapshot. Read-only; fails if the catalog is busy.
pub fn snapshot(catalog: &Catalog, books_dir: &Path) -> io::Result<Snapshot> {
    let mut snap = Snapshot::default();
    let dir = books_dir.to_string_lossy().replace('\'', "''");
    for r in catalog.query(&format!(
        "SELECT p_uuid, p_cdeKey, p_cdeType, p_thumbnail, p_seriesState, p_originType \
         FROM Entries WHERE p_type = 'Entry:Item' AND p_location LIKE '{dir}/%'"
    ))? {
        snap.entries.insert(
            (text(&r["p_cdeKey"]), text(&r["p_cdeType"])),
            Entry {
                uuid: text(&r["p_uuid"]),
                thumbnail: text(&r["p_thumbnail"]),
                series_state: int(&r["p_seriesState"]),
                origin_type: int(&r["p_originType"]),
            },
        );
    }
    for r in catalog.query(
        "SELECT p_uuid, p_cdeKey, p_titles_0_nominal, p_memberCount, p_homeMemberCount, \
         p_isProcessed, p_thumbnail, p_virtualCollectionCount, j_members FROM Entries \
         WHERE p_type = 'Entry:Item:Series' AND p_cdeType = 'series' AND p_cdeKey GLOB 'KW*'",
    )? {
        let key = text(&r["p_cdeKey"]);
        if !is_our_key(&key) {
            continue;
        }
        let members = r["j_members"]
            .as_str()
            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
            .unwrap_or_default();
        snap.containers.insert(
            key,
            Container {
                uuid: text(&r["p_uuid"]),
                title: text(&r["p_titles_0_nominal"]),
                member_count: int(&r["p_memberCount"]).unwrap_or(0),
                home_member_count: int(&r["p_homeMemberCount"]).unwrap_or(0),
                is_processed: int(&r["p_isProcessed"]).unwrap_or(0),
                thumbnail: text(&r["p_thumbnail"]),
                virtual_count: int(&r["p_virtualCollectionCount"]).unwrap_or(0),
                members,
            },
        );
    }
    for r in catalog.query(&format!(
        "SELECT d_seriesId, d_itemCdeKey, d_itemPosition, d_itemPositionLabel FROM Series \
         WHERE d_seriesId GLOB '{SERIES_ID_PREFIX}KW*'"
    ))? {
        let key = text(&r["d_seriesId"])
            .trim_start_matches(SERIES_ID_PREFIX)
            .to_owned();
        if !is_our_key(&key) {
            continue;
        }
        snap.rows.push(Row {
            key,
            cde_key: text(&r["d_itemCdeKey"]),
            position: r["d_itemPosition"].as_f64().unwrap_or(0.0),
            label: text(&r["d_itemPositionLabel"]),
        });
    }
    Ok(snap)
}

/// A series as it should be.
#[derive(Debug)]
struct Wanted<'a> {
    key: String,
    name: &'a str,
    /// In series order, with their catalog rows.
    members: Vec<(&'a Book, &'a Entry)>,
}

/// The commands for one pass.
#[derive(Debug, Default)]
pub struct Plan {
    pub commands: Vec<Value>,
    /// A series member isn't indexed yet: run again later.
    pub pending: bool,
}

fn titles(name: &str) -> Value {
    json!([{"display": name, "collation": name, "pronunciation": name, "language": "en"}])
}

fn credits(authors: &[String]) -> Value {
    authors
        .iter()
        .map(|a| json!({"kind": "Author", "name": {"display": a, "collation": a, "pronunciation": a, "language": "en"}}))
        .collect()
}

/// Works out the commands that bring the catalog's series in line with
/// `books`. Pass no books to remove all of our series.
pub fn plan(books: &[Book], snap: &Snapshot) -> Plan {
    let mut out = Plan::default();

    // What we want: series with at least MIN_MEMBERS indexed books.
    let mut groups: BTreeMap<String, Vec<&Book>> = BTreeMap::new();
    for b in books {
        groups.entry(series_key(&b.series)).or_default().push(b);
    }
    let mut wanted: BTreeMap<String, Wanted> = BTreeMap::new();
    for (key, mut group) in groups {
        group.sort_by(|a, b| {
            a.index
                .total_cmp(&b.index)
                .then_with(|| a.title.cmp(&b.title))
        });
        let mut members = Vec::new();
        for b in group {
            match snap.entries.get(&(b.cde_key.clone(), b.cde_type.clone())) {
                Some(e) => members.push((b, e)),
                None => out.pending = true,
            }
        }
        if members.len() >= MIN_MEMBERS {
            let name = members[0].0.series.as_str();
            wanted.insert(key.clone(), Wanted { key, name, members });
        }
    }
    let wanted_member = |key: &str, cde_key: &str| {
        wanted
            .get(key)
            .is_some_and(|w| w.members.iter().any(|(b, _)| b.cde_key == cde_key))
    };
    let in_any_wanted: HashSet<&str> = wanted
        .values()
        .flat_map(|w| w.members.iter().map(|(b, _)| b.cde_key.as_str()))
        .collect();
    let entry_by_key = |cde_key: &str| {
        snap.entries
            .iter()
            .find(|((k, _), _)| k == cde_key)
            .map(|(_, e)| e)
    };

    // 1. Books leaving a series: show them again, then drop their rows.
    for row in &snap.rows {
        if wanted_member(&row.key, &row.cde_key) {
            continue;
        }
        if !in_any_wanted.contains(row.cde_key.as_str()) {
            if let Some(e) = entry_by_key(&row.cde_key) {
                out.commands.push(
                    json!({"update": {"uuid": e.uuid, "seriesState": 1, "originType": "null"}}),
                );
            }
        }
        out.commands.push(json!({"delete": {"table": "SERIES", "seriesId": series_id(&row.key), "itemCdeKey": row.cde_key}}));
    }
    // 2. Series that are gone: unlink, then delete the container.
    for (key, c) in &snap.containers {
        if wanted.contains_key(key) {
            continue;
        }
        if !c.members.is_empty() {
            out.commands.push(json!({"update": {"uuid": c.uuid, "type": "Entry:Item:Series", "cdeType": "series", "members": []}}));
        }
        out.commands.push(json!({"delete": {"uuid": c.uuid}}));
    }

    // 3. Series we want: rows, container, then hide the members.
    for w in wanted.values() {
        let sid = series_id(&w.key);
        let n = w.members.len() as i64;
        for (b, _) in &w.members {
            let label = index_label(b.index);
            let current = snap
                .rows
                .iter()
                .find(|r| r.key == w.key && r.cde_key == b.cde_key);
            if current.is_none_or(|r| r.position != b.index || r.label != label) {
                out.commands.push(json!({"insertOr": {"table": "SERIES", "onConflict": "REPLACE", "entry": {
                    "seriesId": sid, "itemCdeKey": b.cde_key, "itemType": "Entry:Item",
                    "itemPosition": b.index, "itemPositionLabel": label, "seriesOrderType": "ordered"}}}));
            }
        }
        let (first_book, first_entry) = w.members[0];
        let member_uuids: Vec<&str> = w.members.iter().map(|(_, e)| e.uuid.as_str()).collect();
        let existing = snap.containers.get(&w.key);
        let uuid = existing.map_or_else(|| container_uuid(&w.key), |c| c.uuid.clone());
        if existing.is_none() {
            out.commands
                .push(json!({"insertOr": {"onConflict": "IGNORE", "entry": {
                "type": "Entry:Item:Series", "uuid": uuid, "cdeKey": w.key, "cdeType": "series",
                "cdeGroup": sid, "titles": titles(w.name), "credits": credits(&first_book.authors),
                "displayObjects": [{"ref": "titles"}, {"ref": "credits"}],
                "mimeType": "application/x-kindle-series", "thumbnail": first_entry.thumbnail,
                "isArchived": 1, "isVisibleInHome": 1, "isLatestItem": 1, "originType": -1,
                "virtualCollectionCount": n}}}));
        }
        let up_to_date = existing.is_some_and(|c| {
            c.title == w.name
                && c.member_count == n
                && c.home_member_count == n
                && c.is_processed == 1
                && c.thumbnail == first_entry.thumbnail
                && c.virtual_count == n
                && c.members.iter().map(String::as_str).collect::<HashSet<_>>()
                    == member_uuids.iter().copied().collect()
        });
        if !up_to_date {
            out.commands.push(json!({"update": {
                "uuid": uuid, "type": "Entry:Item:Series", "cdeType": "series",
                "titles": titles(w.name), "credits": credits(&first_book.authors),
                "thumbnail": first_entry.thumbnail, "virtualCollectionCount": n,
                "isProcessed": 1, "members": member_uuids}}));
        }
        for (_, e) in &w.members {
            if e.series_state != Some(0) || e.origin_type != Some(0) {
                out.commands
                    .push(json!({"update": {"uuid": e.uuid, "seriesState": 0, "originType": 0}}));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(key: &str, series: &str, index: f64) -> Book {
        Book {
            cde_key: key.into(),
            cde_type: "EBOK".into(),
            series: series.into(),
            index,
            title: format!("Title {key}"),
            authors: vec!["Jane Author".into()],
        }
    }

    fn entry(uuid: &str, state: i64, origin: Option<i64>) -> Entry {
        Entry {
            uuid: uuid.into(),
            thumbnail: format!("/thumbs/{uuid}.jpg"),
            series_state: Some(state),
            origin_type: origin,
        }
    }

    fn snap(entries: &[(&str, Entry)]) -> Snapshot {
        Snapshot {
            entries: entries
                .iter()
                .map(|(k, e)| (((*k).to_owned(), "EBOK".to_owned()), e.clone()))
                .collect(),
            ..Default::default()
        }
    }

    fn ops(p: &Plan) -> Vec<String> {
        p.commands
            .iter()
            .map(|c| {
                let (op, body) = c.as_object().unwrap().iter().next().unwrap();
                let target = body["table"]
                    .as_str()
                    .map(|_| "series-row".to_owned())
                    .unwrap_or_else(|| {
                        body["entry"]["type"]
                            .as_str()
                            .or(body["type"].as_str())
                            .unwrap_or("book")
                            .to_owned()
                    });
                format!("{op} {target}")
            })
            .collect()
    }

    #[test]
    fn keys_are_ten_characters_and_stable() {
        let k = series_key("The Long Earth");
        assert_eq!(k.len(), 10);
        assert!(is_our_key(&k), "{k}");
        assert_eq!(k, series_key("  the long earth "));
        assert_ne!(k, series_key("The Long Mars"));
        assert!(!is_our_key("B0841YS9HZ"));
        assert!(!is_our_key("KW0000000"));
        let u = container_uuid(&k);
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "5");
        assert_eq!(u, container_uuid(&k));
    }

    #[test]
    fn labels_follow_calibre() {
        assert_eq!(index_label(2.0), "2");
        assert_eq!(index_label(2.5), "2.5");
        assert_eq!(index_label(0.0), "0");
    }

    #[test]
    fn records_without_series_or_key_are_skipped() {
        let records = vec![
            json!({"series": "Saga", "series_index": 2.0, "title": "B", "authors": ["A"], "_kws": {"cde_key": "k1", "cde_type": "EBOK"}}),
            json!({"series": null, "_kws": {"cde_key": "k2", "cde_type": "EBOK"}}),
            json!({"series": "  ", "_kws": {"cde_key": "k3", "cde_type": "EBOK"}}),
            json!({"series": "Saga", "series_index": 1.0}),
        ];
        let books = books_from_records(&records);
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].index, 2.0);
        assert_eq!(books[0].authors, ["A"]);
    }

    #[test]
    fn new_series_creates_rows_then_container_then_hides() {
        let books = [book("k2", "Saga", 2.0), book("k1", "Saga", 1.0)];
        let s = snap(&[("k1", entry("u1", 1, None)), ("k2", entry("u2", 1, None))]);
        let p = plan(&books, &s);
        assert!(!p.pending);
        assert_eq!(
            ops(&p),
            [
                "insertOr series-row",
                "insertOr series-row",
                "insertOr Entry:Item:Series",
                "update Entry:Item:Series",
                "update book",
                "update book"
            ]
        );
        let key = series_key("Saga");
        let rows = &p.commands[..2];
        assert_eq!(rows[0]["insertOr"]["entry"]["itemCdeKey"], "k1");
        assert_eq!(rows[0]["insertOr"]["entry"]["itemPositionLabel"], "1");
        assert_eq!(
            rows[0]["insertOr"]["entry"]["seriesId"],
            format!("urn:collection:1:asin-{key}")
        );
        let c = &p.commands[2]["insertOr"]["entry"];
        assert_eq!(c["cdeKey"], key);
        assert_eq!(c["thumbnail"], "/thumbs/u1.jpg");
        assert_eq!(c["titles"][0]["display"], "Saga");
        assert_eq!(p.commands[3]["update"]["members"], json!(["u1", "u2"]));
        assert_eq!(p.commands[3]["update"]["isProcessed"], 1);
        assert_eq!(
            p.commands[4]["update"],
            json!({"uuid": "u1", "seriesState": 0, "originType": 0})
        );
    }

    #[test]
    fn single_books_and_unindexed_members_make_no_series() {
        let books = [
            book("k1", "Solo", 1.0),
            book("k2", "Pair", 1.0),
            book("k3", "Pair", 2.0),
        ];
        let s = snap(&[("k1", entry("u1", 1, None)), ("k2", entry("u2", 1, None))]);
        let p = plan(&books, &s);
        assert!(p.commands.is_empty());
        assert!(p.pending, "k3 isn't indexed yet");
    }

    /// The catalog as it is after `plan` for Saga (k1, k2) was applied.
    fn applied() -> Snapshot {
        let key = series_key("Saga");
        let mut s = snap(&[
            ("k1", entry("u1", 0, Some(0))),
            ("k2", entry("u2", 0, Some(0))),
        ]);
        s.containers.insert(
            key.clone(),
            Container {
                uuid: container_uuid(&key),
                title: "Saga".into(),
                member_count: 2,
                home_member_count: 2,
                is_processed: 1,
                thumbnail: "/thumbs/u1.jpg".into(),
                virtual_count: 2,
                members: vec!["u1".into(), "u2".into()],
            },
        );
        for (k, i) in [("k1", 1.0), ("k2", 2.0)] {
            s.rows.push(Row {
                key: key.clone(),
                cde_key: k.into(),
                position: i,
                label: index_label(i),
            });
        }
        s
    }

    #[test]
    fn a_settled_series_needs_no_commands() {
        let books = [book("k1", "Saga", 1.0), book("k2", "Saga", 2.0)];
        assert!(plan(&books, &applied()).commands.is_empty());
    }

    #[test]
    fn firmware_drift_is_repaired() {
        // After a re-index (originType reset) and a delete's post-processing
        // (members shown again, cloud count zeroed).
        let books = [book("k1", "Saga", 1.0), book("k2", "Saga", 2.0)];
        let mut s = applied();
        s.entries
            .get_mut(&("k1".into(), "EBOK".into()))
            .unwrap()
            .origin_type = None;
        s.entries
            .get_mut(&("k2".into(), "EBOK".into()))
            .unwrap()
            .series_state = Some(1);
        s.containers.values_mut().next().unwrap().member_count = 0;
        let p = plan(&books, &s);
        assert_eq!(
            ops(&p),
            ["update Entry:Item:Series", "update book", "update book"]
        );
    }

    #[test]
    fn a_deleted_member_dissolves_a_pair_and_drops_its_orphan_row() {
        // k2's file is gone: no entry, but its Series row is left behind.
        let books = [book("k1", "Saga", 1.0)];
        let mut s = applied();
        s.entries.remove(&("k2".into(), "EBOK".into()));
        let p = plan(&books, &s);
        assert_eq!(
            ops(&p),
            [
                "update book",
                "delete series-row",
                "delete series-row",
                "update Entry:Item:Series",
                // `delete` by uuid names no type.
                "delete book"
            ]
        );
        assert_eq!(
            p.commands[0]["update"],
            json!({"uuid": "u1", "seriesState": 1, "originType": "null"})
        );
        assert_eq!(p.commands[3]["update"]["members"], json!([]));
    }

    #[test]
    fn a_book_moving_series_is_unhidden_only_if_it_is_not_wanted_elsewhere() {
        let books = [
            book("k1", "Saga", 1.0),
            book("k2", "Other", 1.0),
            book("k3", "Saga", 2.0),
            book("k4", "Other", 2.0),
        ];
        let mut s = applied();
        s.entries
            .insert(("k3".into(), "EBOK".into()), entry("u3", 1, None));
        s.entries
            .insert(("k4".into(), "EBOK".into()), entry("u4", 1, None));
        let p = plan(&books, &s);
        let c = &p.commands;
        // k2's old row goes, without showing k2 (it's about to join "Other").
        assert!(c[0]["delete"]["itemCdeKey"] == "k2", "{c:?}");
        assert!(!c
            .iter()
            .any(|x| x["update"]["uuid"] == "u2" && x["update"]["seriesState"] == 1));
        // k2 is still hidden from Saga, so it needs no update; k4 joins
        // "Other" and is hidden; the new series gets both members.
        assert!(c
            .iter()
            .any(|x| x["update"]["uuid"] == "u4" && x["update"]["seriesState"] == 0));
        let other = c
            .iter()
            .find(|x| x["update"]["titles"][0]["display"] == "Other")
            .unwrap();
        assert_eq!(other["update"]["members"], json!(["u2", "u4"]));
    }

    #[test]
    fn removing_everything_unhides_first() {
        let p = plan(&[], &applied());
        assert_eq!(
            ops(&p),
            [
                "update book",
                "delete series-row",
                "update book",
                "delete series-row",
                "update Entry:Item:Series",
                "delete book"
            ]
        );
    }

    #[test]
    fn reordering_rewrites_only_changed_rows() {
        let books = [book("k1", "Saga", 1.0), book("k2", "Saga", 2.5)];
        let p = plan(&books, &applied());
        assert_eq!(ops(&p), ["insertOr series-row"]);
        assert_eq!(
            p.commands[0]["insertOr"]["entry"]["itemPositionLabel"],
            "2.5"
        );
    }
}
