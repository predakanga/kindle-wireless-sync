// SPDX-License-Identifier: GPL-3.0-or-later
//! Adopting books that Calibre sent over USB (ADR 0013).
//!
//! Calibre's wireless connection only knows the books we report, so a book
//! its USB driver sent looks absent and would be sent again as a duplicate.
//! At the start of each session we look for book files we have no record of
//! and, if they carry a Calibre UUID, record them with metadata from the USB
//! driver's `metadata.calibre` or, failing that, from the file's EXTH header.
//! Nothing on disk is changed.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::kindle;
use crate::lpath;

/// The USB driver's record of the books it sent, at the root of the
/// device's storage (`/mnt/us`), next to the books directory.
pub const USB_METADATA_FILE: &str = "metadata.calibre";

/// How deep to look below the books directory.
const MAX_DEPTH: u32 = 4;

/// A book file we have no record of.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Relative to the books directory, `/`-separated: our lpath for it.
    pub lpath: String,
    pub path: PathBuf,
    pub size: u64,
    pub mtime: SystemTime,
}

impl Candidate {
    fn extension(&self) -> String {
        self.lpath
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default()
    }

    fn stem(&self) -> &str {
        let name = self.lpath.rsplit('/').next().unwrap_or(&self.lpath);
        name.rsplit_once('.').map_or(name, |(s, _)| s)
    }
}

/// Files a scan looked at and didn't adopt, so they aren't read again while
/// their size and modification time stay the same. Kept in memory only.
#[derive(Debug, Default)]
pub struct Memo {
    rejected: HashMap<String, (u64, SystemTime)>,
}

impl Memo {
    pub fn reject(&mut self, c: &Candidate) {
        self.rejected.insert(c.lpath.clone(), (c.size, c.mtime));
    }

    fn rejected(&self, c: &Candidate) -> bool {
        self.rejected.get(&c.lpath) == Some(&(c.size, c.mtime))
    }
}

/// Book files under `books_dir` with an accepted extension, a valid lpath,
/// no record in `known`, and not already rejected. Skips dot-files and
/// `.sdr` sidecar folders.
pub fn find_unknown(
    books_dir: &Path,
    known: &HashSet<String>,
    accepted: &[String],
    memo: &Memo,
) -> Vec<Candidate> {
    let mut found = Vec::new();
    walk(books_dir, "", MAX_DEPTH, &mut |path, lpath, meta| {
        let ext = lpath
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        if !accepted.contains(&ext)
            || known.contains(lpath)
            || lpath::validate(lpath, Some(accepted)).is_err()
        {
            return;
        }
        let c = Candidate {
            lpath: lpath.to_owned(),
            path: path.to_path_buf(),
            size: meta.len(),
            mtime: meta.modified().unwrap_or(UNIX_EPOCH),
        };
        if !memo.rejected(&c) {
            found.push(c);
        }
    });
    found
}

fn walk(dir: &Path, prefix: &str, depth: u32, f: &mut dyn FnMut(&Path, &str, &fs::Metadata)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') {
            continue;
        }
        let lpath = format!("{prefix}{name}");
        let path = entry.path();
        // Don't follow symlinks: stay inside the books directory.
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            if depth > 1 && !name.to_ascii_lowercase().ends_with(".sdr") {
                walk(&path, &format!("{lpath}/"), depth - 1, f);
            }
        } else if meta.is_file() {
            f(&path, &lpath, &meta);
        }
    }
}

/// The USB driver's entries that have a Calibre UUID, keyed by their lpath
/// relative to the books directory and lowercased. Its lpaths are relative to
/// the storage root (`documents/...`) and lowercased by the MTP driver, so
/// `books_prefix` is the books directory's name plus `/`. A missing or
/// unreadable file gives an empty map.
pub fn read_usb_metadata(path: &Path, books_prefix: &str) -> HashMap<String, Value> {
    let Some(Value::Array(entries)) = fs::read(path)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
    else {
        return HashMap::new();
    };
    let prefix = books_prefix.to_lowercase();
    entries
        .into_iter()
        .filter(|e| e["uuid"].as_str().is_some_and(kindle::is_uuid))
        .filter_map(|e| {
            let l = e["lpath"].as_str()?.to_lowercase();
            Some((l.strip_prefix(&prefix)?.to_owned(), e))
        })
        .collect()
}

/// Where an adopted record's metadata came from, for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    UsbMetadata,
    Exth,
}

/// The record to adopt `c` with, or `None` if it has no Calibre UUID. `head`
/// is the start of the file for MOBI-family formats (see
/// [`kindle::HEAD_LEN`]).
pub fn record_for(
    c: &Candidate,
    usb: &HashMap<String, Value>,
    head: Option<&[u8]>,
) -> Option<(Value, Source)> {
    let from_usb = usb
        .get(&c.lpath.to_lowercase())
        .filter(|e| e["size"].as_u64() == Some(c.size));
    let (mut record, source) = match from_usb {
        Some(e) => (e.clone(), Source::UsbMetadata),
        None => {
            let id = kindle::read_calibre_identity(head?)?;
            let authors = if id.authors.is_empty() {
                vec!["Unknown".to_owned()]
            } else {
                id.authors
            };
            let title = id.title.unwrap_or_else(|| c.stem().to_owned());
            (
                json!({"title": title, "authors": authors, "uuid": id.uuid}),
                Source::Exth,
            )
        }
    };
    record["lpath"] = json!(c.lpath);
    record["size"] = json!(c.size);
    record["last_modified"] = json!(iso_utc(c.mtime));
    record["thumbnail"] = Value::Null;
    Some((record, source))
}

/// Whether we can read a Calibre identity from this file's header.
pub fn is_mobi_family(c: &Candidate) -> bool {
    matches!(c.extension().as_str(), "azw3" | "mobi" | "azw" | "prc")
}

/// Formats a time as Calibre's JSON does: ISO 8601 in UTC.
pub fn iso_utc(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}+00:00",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kindle::tests::fake_mobi;
    use std::time::Duration;

    const UUID: &str = "cb5d2673-dbcf-4a0c-b9a6-90de29a24177";

    fn exts() -> Vec<String> {
        ["azw3", "mobi", "azw", "kfx", "pdf", "txt"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn candidate(lpath: &str, size: u64) -> Candidate {
        Candidate {
            lpath: lpath.into(),
            path: PathBuf::from(lpath),
            size,
            mtime: UNIX_EPOCH + Duration::from_secs(1_792_060_376),
        }
    }

    #[test]
    fn formats_times_like_calibre() {
        assert_eq!(iso_utc(UNIX_EPOCH), "1970-01-01T00:00:00+00:00");
        assert_eq!(
            iso_utc(UNIX_EPOCH + Duration::from_secs(1_792_060_376)),
            "2026-10-15T10:32:56+00:00"
        );
        assert_eq!(
            iso_utc(UNIX_EPOCH + Duration::from_secs(951_782_400)),
            "2000-02-29T00:00:00+00:00"
        );
    }

    #[test]
    fn finds_only_unknown_accepted_books() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        for f in [
            "Known.azw3",
            "New.azw3",
            "Book.epub",
            ".hidden.azw3",
            "Author/Nested.mobi",
            "New.sdr/inside.azw3",
            "Downloads/Items01/dict.azw",
        ] {
            let p = d.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, b"x").unwrap();
        }
        let known: HashSet<String> = ["Known.azw3".to_string()].into();
        let mut got: Vec<String> = find_unknown(d, &known, &exts(), &Memo::default())
            .into_iter()
            .map(|c| c.lpath)
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                "Author/Nested.mobi",
                "Downloads/Items01/dict.azw",
                "New.azw3"
            ]
        );
    }

    #[test]
    fn rejected_files_are_skipped_until_they_change() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("A.azw3"), b"x").unwrap();
        let mut memo = Memo::default();
        let found = find_unknown(tmp.path(), &HashSet::new(), &exts(), &memo);
        memo.reject(&found[0]);
        assert!(find_unknown(tmp.path(), &HashSet::new(), &exts(), &memo).is_empty());
        fs::write(tmp.path().join("A.azw3"), b"changed").unwrap();
        assert_eq!(
            find_unknown(tmp.path(), &HashSet::new(), &exts(), &memo).len(),
            1
        );
    }

    #[test]
    fn reads_calibre_identity_from_exth() {
        let book = fake_mobi(&[
            (100, b"Jane Author"),
            (100, b"John Writer"),
            (503, b"Example Book"),
            (112, format!("calibre:{UUID}").as_bytes()),
            (113, b"B0053VMNYW"),
            (501, b"EBOK"),
        ]);
        let (record, source) =
            record_for(&candidate("Example.azw3", 9), &HashMap::new(), Some(&book)).unwrap();
        assert_eq!(source, Source::Exth);
        assert_eq!(record["uuid"], UUID);
        assert_eq!(record["title"], "Example Book");
        assert_eq!(record["authors"], json!(["Jane Author", "John Writer"]));
        assert_eq!(record["lpath"], "Example.azw3");
        assert_eq!(record["size"], 9);
        assert_eq!(record["last_modified"], "2026-10-15T10:32:56+00:00");
    }

    #[test]
    fn uuid_shaped_asin_is_enough_and_title_falls_back_to_the_file_name() {
        let book = fake_mobi(&[(113, UUID.as_bytes()), (501, b"EBOK")]);
        let (record, _) = record_for(
            &candidate("Dir/My Book.mobi", 1),
            &HashMap::new(),
            Some(&book),
        )
        .unwrap();
        assert_eq!(record["uuid"], UUID);
        assert_eq!(record["title"], "My Book");
        assert_eq!(record["authors"], json!(["Unknown"]));
    }

    #[test]
    fn amazon_books_and_non_mobi_files_are_not_adopted() {
        let amazon = fake_mobi(&[(113, b"B0053VMNYW"), (501, b"EBOK"), (503, b"Dictionary")]);
        let usb = HashMap::new();
        assert!(record_for(&candidate("dict.azw", 1), &usb, Some(&amazon)).is_none());
        assert!(record_for(&candidate("doc.pdf", 1), &usb, None).is_none());
        assert!(record_for(&candidate("bad.azw3", 1), &usb, Some(b"not a mobi")).is_none());
    }

    #[test]
    fn usb_metadata_wins_when_the_size_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(USB_METADATA_FILE);
        fs::write(
            &path,
            json!([
                {"lpath": "documents/example book - jane author.azw3", "uuid": UUID,
                 "title": "Example Book", "authors": ["Jane Author"], "series": "Saga",
                 "size": 5, "application_id": 42, "thumbnail": [1, 1, "abc"]},
                {"lpath": "documents/report.pdf", "uuid": UUID, "title": "Report", "size": 7},
                {"lpath": "koreader/readme.md", "uuid": null, "size": 3},
                {"lpath": "documents/no uuid.azw3", "size": 3}
            ])
            .to_string(),
        )
        .unwrap();
        let usb = read_usb_metadata(&path, "documents/");
        let mut keys: Vec<&String> = usb.keys().collect();
        keys.sort();
        assert_eq!(keys, ["example book - jane author.azw3", "report.pdf"]);

        let c = candidate("Example Book - Jane Author.azw3", 5);
        let (record, source) = record_for(&c, &usb, None).unwrap();
        assert_eq!(source, Source::UsbMetadata);
        assert_eq!(record["series"], "Saga");
        assert_eq!(record["application_id"], 42);
        assert_eq!(record["lpath"], "Example Book - Jane Author.azw3");
        assert_eq!(record["thumbnail"], Value::Null);
        assert!(record_for(&candidate("Report.pdf", 7), &usb, None).is_some());

        // A replaced file (different size) falls back to its own header.
        let book = fake_mobi(&[(112, format!("calibre:{UUID}").as_bytes())]);
        let (_, source) = record_for(
            &candidate("Example Book - Jane Author.azw3", 6),
            &usb,
            Some(&book),
        )
        .unwrap();
        assert_eq!(source, Source::Exth);
        assert!(record_for(&candidate("Report.pdf", 8), &usb, None).is_none());
    }

    #[test]
    fn missing_usb_metadata_is_empty() {
        assert!(
            read_usb_metadata(Path::new("/nonexistent/metadata.calibre"), "documents/").is_empty()
        );
    }
}
