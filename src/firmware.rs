// SPDX-License-Identifier: GPL-3.0-or-later
//! The Kindle's firmware version, and what it allows (ADR 0026).
//!
//! The version is the first line of `/etc/prettyversion.txt` that reads
//! `Kindle X.Y.Z (…)`, as KOReader's `koreader.sh` reads it. A Kindle that
//! updates its firmware reboots, so reading it once at start is enough.

use std::cmp::Ordering;
use std::path::Path;

pub const PRETTY_VERSION: &str = "/etc/prettyversion.txt";

/// The first firmware whose catalog has `Entries.p_lastOpenTime`
/// (migration 81), which read sync needs.
pub const READ_SYNC_MIN: &str = "5.19.1";

/// The version in `prettyversion.txt`'s text, such as `5.20.1`.
pub fn parse(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let version = line.strip_prefix("Kindle ")?.split_whitespace().next()?;
        components(version).map(|_| version.to_owned())
    })
}

/// The version of the firmware running here, or `None` off a Kindle.
pub fn read() -> Option<String> {
    read_from(Path::new(PRETTY_VERSION))
}

pub fn read_from(path: &Path) -> Option<String> {
    parse(&std::fs::read_to_string(path).ok()?)
}

fn components(version: &str) -> Option<Vec<u32>> {
    version.split('.').map(|n| n.parse().ok()).collect()
}

/// Compares two versions number by number; a missing number counts as 0,
/// so `5.19` and `5.19.0` are equal. `None` if either isn't a version.
pub fn compare(a: &str, b: &str) -> Option<Ordering> {
    let (a, b) = (components(a)?, components(b)?);
    let len = a.len().max(b.len());
    let at = |v: &[u32], i: usize| v.get(i).copied().unwrap_or(0);
    Some(
        (0..len)
            .map(|i| at(&a, i).cmp(&at(&b, i)))
            .find(|o| o.is_ne())
            .unwrap_or(Ordering::Equal),
    )
}

/// Whether read sync can run on this firmware. An unknown version allows
/// it: only a version that is read and is older turns it off.
pub fn read_sync_available(firmware: Option<&str>) -> bool {
    firmware.is_none_or(|v| compare(v, READ_SYNC_MIN) != Some(Ordering::Less))
}

/// Why read sync can't be turned on, for the API and the command line.
pub fn read_sync_unavailable(firmware: &str) -> String {
    format!(
        "Reading status sync needs Kindle firmware {READ_SYNC_MIN} or later. \
         This Kindle has {firmware}."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_version_line() {
        let text = "Kindle 5.20.1 (4729740010)\n";
        assert_eq!(parse(text).as_deref(), Some("5.20.1"));
        assert_eq!(
            parse("\nKindle 5.17.1.0.2 (123)").as_deref(),
            Some("5.17.1.0.2")
        );
        assert_eq!(parse("Kindle 5.18.6").as_deref(), Some("5.18.6"));
        assert_eq!(parse("Something else\n"), None);
        assert_eq!(parse("Kindle Paperwhite (123)"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn compares_number_by_number() {
        assert_eq!(compare("5.17.1.0.2", "5.19.1"), Some(Ordering::Less));
        assert_eq!(compare("5.19.1", "5.19.2"), Some(Ordering::Less));
        assert_eq!(compare("5.19.10", "5.19.2"), Some(Ordering::Greater));
        assert_eq!(compare("5.19", "5.19.0"), Some(Ordering::Equal));
        assert_eq!(compare("5.19.2.0.1", "5.19.2"), Some(Ordering::Greater));
        assert_eq!(compare("5.x", "5.19.1"), None);
    }

    #[test]
    fn read_sync_from_5_19_1_or_an_unknown_version() {
        assert!(!read_sync_available(Some("5.18.6")));
        assert!(!read_sync_available(Some("5.16.3")));
        assert!(read_sync_available(Some("5.19.1")));
        assert!(read_sync_available(Some("5.19.2.0.1")));
        assert!(read_sync_available(Some("5.20.1")));
        assert!(read_sync_available(None));
    }

    #[test]
    fn reads_a_missing_file_as_unknown() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_from(&dir.path().join("prettyversion.txt")), None);
        std::fs::write(dir.path().join("prettyversion.txt"), "Kindle 5.18.6 (1)\n").unwrap();
        assert_eq!(
            read_from(&dir.path().join("prettyversion.txt")).as_deref(),
            Some("5.18.6")
        );
    }
}
