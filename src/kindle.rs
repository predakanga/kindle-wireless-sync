// SPDX-License-Identifier: GPL-3.0-or-later
//! Kindle firmware specifics: the content key the scanner gives a book, and
//! cover thumbnails.
//!
//! The Library shows `system/thumbnails/thumbnail_<cdeKey>_<cdeType>_portrait.jpg`
//! for MOBI-family books. The scanner's own thumbnail is only a "No image
//! available" placeholder, but it never overwrites an existing file, so we
//! write Calibre's cover there *before* the book appears (ADR 0005). PDFs and
//! TXTs have no thumbnail link in the catalog, so we give them no cover. See
//! `docs/research/kindle-library-integration.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The scanner's identity for a book: `p_cdeKey` and `p_cdeType` in the
/// catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookKey {
    pub cde_key: String,
    pub cde_type: String,
}

impl BookKey {
    pub fn thumbnail_name(&self) -> String {
        format!("thumbnail_{}_{}_portrait.jpg", self.cde_key, self.cde_type)
    }
}

/// How we can find out a book's key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// Read from the file before it is moved into place.
    Known(BookKey),
    /// Only available from the catalog once the scanner has indexed the file
    /// (KFX, or MOBI files without EXTH 113/501).
    AfterIndex,
    /// The format has no cover thumbnail on the device (PDF, TXT).
    NoCover,
}

/// Works out the key for a book with extension `ext` from the start of its
/// file (`head`, which must include the whole first PalmDB record).
pub fn key_source(ext: &str, head: &[u8]) -> KeySource {
    match ext.to_ascii_lowercase().as_str() {
        "azw3" | "mobi" | "azw" | "prc" => match read_exth_key(head) {
            Some(key) => KeySource::Known(key),
            None => KeySource::AfterIndex,
        },
        "kfx" => KeySource::AfterIndex,
        _ => KeySource::NoCover,
    }
}

/// Bytes of a MOBI file to read to be sure of having its EXTH header.
pub const HEAD_LEN: usize = 256 * 1024;

const EXTH_AUTHOR: u32 = 100;
const EXTH_SOURCE: u32 = 112;
const EXTH_ASIN: u32 = 113;
const EXTH_CDETYPE: u32 = 501;
const EXTH_TITLE: u32 = 503;

fn be_u32(data: &[u8], at: usize) -> Option<u32> {
    let b = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn be_u16(data: &[u8], at: usize) -> Option<u16> {
    let b = data.get(at..at.checked_add(2)?)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

/// The EXTH records of a MOBI/AZW3 PalmDB file, in file order, as
/// `(type, value)`. `None` if the file has no readable EXTH header.
fn exth_records(data: &[u8]) -> Option<Vec<(u32, &[u8])>> {
    // PalmDB header: type/creator "BOOKMOBI" at 60, record count at 76,
    // record list (8 bytes each) from 78.
    if data.get(60..68)? != b"BOOKMOBI" || be_u16(data, 76)? == 0 {
        return None;
    }
    let rec0 = be_u32(data, 78)? as usize;
    // Record 0: 16-byte PalmDOC header, then the MOBI header.
    let mobi = rec0.checked_add(16)?;
    if data.get(mobi..mobi + 4)? != b"MOBI" {
        return None;
    }
    let mobi_len = be_u32(data, mobi + 4)? as usize;
    let exth_flags = be_u32(data, mobi + 0x70)?;
    if exth_flags & 0x40 == 0 {
        return None;
    }
    let exth = mobi.checked_add(mobi_len)?;
    if data.get(exth..exth + 4)? != b"EXTH" {
        return None;
    }
    let count = be_u32(data, exth + 8)?;
    let mut records = Vec::new();
    let mut at = exth + 12;
    for _ in 0..count {
        let kind = be_u32(data, at)?;
        let len = be_u32(data, at + 4)? as usize;
        if len < 8 {
            return None;
        }
        records.push((kind, data.get(at + 8..at.checked_add(len)?)?));
        at += len;
    }
    Some(records)
}

fn exth_text(value: &[u8]) -> Option<&str> {
    std::str::from_utf8(value).ok().map(str::trim)
}

/// Reads EXTH records 113 (ASIN; Calibre writes the book UUID) and 501
/// (cdetype) from a MOBI/AZW3 PalmDB file. Mirrors Calibre's
/// `devices/kindle/driver.py:thumbnail_filename`: both must be present.
pub fn read_exth_key(data: &[u8]) -> Option<BookKey> {
    let (mut asin, mut cdetype) = (None, None);
    for (kind, value) in exth_records(data)? {
        match kind {
            EXTH_ASIN => asin = exth_text(value).map(str::to_owned),
            EXTH_CDETYPE => cdetype = exth_text(value).map(str::to_owned),
            _ => {}
        }
    }
    match (asin, cdetype) {
        (Some(k), Some(t)) if valid_key_part(&k) && valid_key_part(&t) => Some(BookKey {
            cde_key: k,
            cde_type: t,
        }),
        _ => None,
    }
}

/// What Calibre wrote into a MOBI/AZW3 file it made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalibreIdentity {
    pub uuid: String,
    pub title: Option<String>,
    pub authors: Vec<String>,
}

/// Reads a book's Calibre UUID (EXTH 112 `calibre:<uuid>`, else a
/// UUID-shaped EXTH 113), title (503) and authors (100, one record each).
/// `None` without a Calibre UUID, as for Amazon books, whose 113 is an ASIN
/// (ADR 0013).
pub fn read_calibre_identity(data: &[u8]) -> Option<CalibreIdentity> {
    let (mut source, mut asin, mut title, mut authors) = (None, None, None, Vec::new());
    for (kind, value) in exth_records(data)? {
        let Some(text) = exth_text(value).filter(|t| !t.is_empty()) else {
            continue;
        };
        match kind {
            EXTH_SOURCE => source = text.strip_prefix("calibre:").map(str::to_owned),
            EXTH_ASIN => asin = Some(text.to_owned()),
            EXTH_TITLE => title = Some(text.to_owned()),
            EXTH_AUTHOR => authors.push(text.to_owned()),
            _ => {}
        }
    }
    let uuid = source
        .filter(|u| is_uuid(u))
        .or(asin.filter(|u| is_uuid(u)))?;
    Some(CalibreIdentity {
        uuid,
        title,
        authors,
    })
}

/// A UUID in its usual text form, as Calibre gives every book.
pub fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// Keys become part of a file name; keep them to a safe character set.
fn valid_key_part(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_*".contains(&b))
}

/// Asks the catalog for the key the scanner gave the file at `location`.
/// Read-only: we never write to the catalog (ADR 0005).
pub fn lookup_indexed_key(catalog_db: &Path, location: &str) -> Option<BookKey> {
    let sql = format!(
        "select p_cdeKey || '|' || p_cdeType from Entries where p_location = '{}' limit 1",
        location.replace('\'', "''")
    );
    let out = Command::new("sqlite3")
        .arg("-readonly")
        .arg(catalog_db)
        .arg(sql)
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let (k, t) = text.trim().split_once('|')?;
    (valid_key_part(k) && valid_key_part(t)).then(|| BookKey {
        cde_key: k.into(),
        cde_type: t.into(),
    })
}

/// Calls `on_change` each time the Kindle's catalog announces a change
/// (`com.lab126.ccat` sends `updated`, about when the scanner adds a book:
/// research note, "Covers"), from a background thread, for the life of the
/// process. Restarts the event listener if it exits.
pub fn watch_catalog(on_change: impl Fn() + Send + 'static) {
    std::thread::spawn(move || loop {
        let child = Command::new("lipc-wait-event")
            .args(["-m", "com.lab126.ccat", "updated"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn();
        match child {
            Ok(mut child) => {
                let stdout = child.stdout.take().expect("piped stdout");
                for line in std::io::BufRead::lines(std::io::BufReader::new(stdout)) {
                    if line.is_err() {
                        break;
                    }
                    on_change();
                }
                let _ = child.wait();
                log::warn!("lipc-wait-event for the catalog exited; restarting");
            }
            Err(e) => log::warn!("cannot run lipc-wait-event: {e}"),
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
    });
}

/// Longest device name we report to Calibre, in characters.
const MAX_DEVICE_NAME: usize = 64;

/// The name the user gave the Kindle (*Settings → Device Options → Device
/// Name*), cleaned up for Calibre (ADR 0015). Read-only. `None` off a
/// Kindle, or if the name is empty.
pub fn device_name() -> Option<String> {
    let out = Command::new("lipc-get-prop")
        .args(["com.lab126.amazonRegistrationService", "GetDeviceName"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    clean_device_name(&String::from_utf8_lossy(&out.stdout))
}

/// Trims, drops control characters and caps the length. `None` if nothing
/// is left.
pub fn clean_device_name(raw: &str) -> Option<String> {
    let name: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_DEVICE_NAME)
        .collect();
    let name = name.trim_end().to_owned();
    (!name.is_empty()).then_some(name)
}

/// Where covers go on the device, and where we keep our backups.
#[derive(Debug, Clone)]
pub struct CoverConfig {
    /// `/mnt/us/system/thumbnails` on the device.
    pub thumbnails_dir: PathBuf,
    /// `/var/local/cc.db`, for keys only known after indexing. `None`
    /// disables that fallback.
    pub catalog_db: Option<PathBuf>,
}

/// Decodes standard base64 (Calibre's `as_base64_unicode`), ignoring
/// whitespace.
pub fn decode_base64(input: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    }
    let clean: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let body = clean
        .strip_suffix(b"==")
        .or_else(|| clean.strip_suffix(b"="))
        .unwrap_or(&clean);
    if !clean.len().is_multiple_of(4) || body.contains(&b'=') {
        return None;
    }
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    for chunk in body.chunks(4) {
        let mut acc = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            acc |= val(c)? << (18 - 6 * i);
        }
        let bytes = acc.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Some(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds a minimal PalmDB/MOBI file with the given EXTH records.
    pub(crate) fn fake_mobi(exth: &[(u32, &[u8])]) -> Vec<u8> {
        let mut f = vec![0u8; 78];
        f[60..68].copy_from_slice(b"BOOKMOBI");
        f[76..78].copy_from_slice(&1u16.to_be_bytes());
        let rec0 = 78 + 8 + 2;
        f.extend_from_slice(&(rec0 as u32).to_be_bytes());
        f.extend_from_slice(&[0; 4]);
        f.extend_from_slice(&[0; 2]);
        f.extend_from_slice(&[0; 16]); // PalmDOC header
        let mobi_len = 0xE8u32;
        let mut mobi = vec![0u8; mobi_len as usize];
        mobi[0..4].copy_from_slice(b"MOBI");
        mobi[4..8].copy_from_slice(&mobi_len.to_be_bytes());
        mobi[0x70..0x74].copy_from_slice(&0x50u32.to_be_bytes());
        f.extend_from_slice(&mobi);
        let mut recs = Vec::new();
        for (kind, value) in exth {
            recs.extend_from_slice(&kind.to_be_bytes());
            recs.extend_from_slice(&((value.len() + 8) as u32).to_be_bytes());
            recs.extend_from_slice(value);
        }
        f.extend_from_slice(b"EXTH");
        f.extend_from_slice(&((recs.len() + 12) as u32).to_be_bytes());
        f.extend_from_slice(&(exth.len() as u32).to_be_bytes());
        f.extend_from_slice(&recs);
        f.extend_from_slice(b"rest of the book");
        f
    }

    #[test]
    fn reads_calibre_style_exth_key() {
        let f = fake_mobi(&[
            (100, b"Author"),
            (113, b"cb5d2673-dbcf-4a0c-b9a6-90de29a24177"),
            (501, b"EBOK"),
        ]);
        let key = read_exth_key(&f).unwrap();
        assert_eq!(key.cde_key, "cb5d2673-dbcf-4a0c-b9a6-90de29a24177");
        assert_eq!(key.cde_type, "EBOK");
        assert_eq!(
            key.thumbnail_name(),
            "thumbnail_cb5d2673-dbcf-4a0c-b9a6-90de29a24177_EBOK_portrait.jpg"
        );
        assert_eq!(key_source("AZW3", &f), KeySource::Known(key));
    }

    #[test]
    fn missing_or_bad_records_fall_back() {
        assert_eq!(read_exth_key(&fake_mobi(&[(113, b"abc")])), None);
        assert_eq!(
            read_exth_key(&fake_mobi(&[(113, b"../x"), (501, b"EBOK")])),
            None
        );
        let f = fake_mobi(&[(113, b"abc"), (501, b"EBOK")]);
        assert_eq!(read_exth_key(&f[..200]), None, "truncated");
        assert_eq!(read_exth_key(b"not a palm database at all"), None);
        assert_eq!(key_source("mobi", b"junk"), KeySource::AfterIndex);
        assert_eq!(key_source("kfx", &f), KeySource::AfterIndex);
        assert_eq!(key_source("pdf", &f), KeySource::NoCover);
        assert_eq!(key_source("txt", &f), KeySource::NoCover);
    }

    #[test]
    fn hostile_record_lengths_do_not_panic() {
        let mut f = fake_mobi(&[(113, b"abc"), (501, b"EBOK")]);
        let exth = f.windows(4).position(|w| w == b"EXTH").unwrap();
        f[exth + 16..exth + 20].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(read_exth_key(&f), None);
        f[exth + 8..exth + 12].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(read_exth_key(&f), None);
    }

    #[test]
    fn cleans_device_names() {
        assert_eq!(
            clean_device_name("Jane's Kindle\n").as_deref(),
            Some("Jane's Kindle")
        );
        assert_eq!(
            clean_device_name("  Tab\there\u{7}  ").as_deref(),
            Some("Tabhere")
        );
        assert_eq!(clean_device_name(" \n\t "), None);
        let long = "é".repeat(100);
        assert_eq!(clean_device_name(&long).unwrap().chars().count(), 64);
        // A cut that lands after a space doesn't leave it dangling.
        let spaced = format!("{} b", "a".repeat(63));
        assert_eq!(clean_device_name(&spaced).unwrap(), "a".repeat(63));
    }

    #[test]
    fn decodes_base64() {
        assert_eq!(decode_base64("").unwrap(), b"");
        assert_eq!(decode_base64("Zg==").unwrap(), b"f");
        assert_eq!(decode_base64("Zm8=").unwrap(), b"fo");
        assert_eq!(decode_base64("Zm9v").unwrap(), b"foo");
        assert_eq!(
            decode_base64("/9j/\n4AAQ").unwrap(),
            [0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10]
        );
        assert_eq!(decode_base64("Zm9"), None);
        assert_eq!(decode_base64("Z=9v"), None);
        assert_eq!(decode_base64("Zm9!"), None);
    }
}
