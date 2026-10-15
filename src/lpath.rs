//! Validation of `lpath`s supplied by Calibre.
//!
//! Calibre names every book by an `lpath`, a `/`-separated path relative to
//! the device's books directory. The server is not trusted (ADR 0003), so an
//! lpath is accepted only if it cannot escape the books directory or clash
//! with files we or the firmware own.

use std::fmt;
use std::path::PathBuf;

/// Longest single path component, in bytes (ext4 `NAME_MAX`).
const MAX_COMPONENT_LEN: usize = 255;
/// Longest whole lpath, in bytes.
const MAX_LPATH_LEN: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LpathError {
    Empty,
    TooLong,
    Absolute,
    BadCharacter(char),
    BadComponent(String),
    ExtensionNotAllowed(String),
}

impl fmt::Display for LpathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LpathError::Empty => write!(f, "empty path"),
            LpathError::TooLong => write!(f, "path too long"),
            LpathError::Absolute => write!(f, "absolute path"),
            LpathError::BadCharacter(c) => write!(f, "forbidden character {c:?}"),
            LpathError::BadComponent(c) => write!(f, "forbidden path component {c:?}"),
            LpathError::ExtensionNotAllowed(e) => write!(f, "file type {e:?} not accepted"),
        }
    }
}

impl std::error::Error for LpathError {}

/// Checks `lpath` and returns it as a relative path.
///
/// `allowed_extensions` are lowercase and without the dot. Pass `None` to
/// skip the extension check (e.g. when deleting a book we already track).
pub fn validate(lpath: &str, allowed_extensions: Option<&[String]>) -> Result<PathBuf, LpathError> {
    if lpath.is_empty() {
        return Err(LpathError::Empty);
    }
    if lpath.len() > MAX_LPATH_LEN {
        return Err(LpathError::TooLong);
    }
    if lpath.starts_with('/') {
        return Err(LpathError::Absolute);
    }
    if let Some(c) = lpath
        .chars()
        .find(|c| *c == '\\' || *c == '\0' || c.is_control())
    {
        return Err(LpathError::BadCharacter(c));
    }
    // Windows drive prefixes ("C:") are meaningless here and suggest a
    // confused or hostile server.
    if lpath.as_bytes().get(1) == Some(&b':') {
        return Err(LpathError::Absolute);
    }

    let components: Vec<&str> = lpath.split('/').collect();
    for c in &components {
        // Leading dots cover "." and "..", hidden files and our own
        // ".kws-*" temporary files.
        if c.is_empty() || c.starts_with('.') || c.len() > MAX_COMPONENT_LEN {
            return Err(LpathError::BadComponent((*c).to_string()));
        }
    }
    let file_name = components.last().unwrap();
    // The firmware keeps per-book state in "<name>.sdr" directories.
    if components
        .iter()
        .any(|c| c.to_ascii_lowercase().ends_with(".sdr"))
    {
        return Err(LpathError::BadComponent(file_name.to_string()));
    }

    if let Some(allowed) = allowed_extensions {
        let ext = match file_name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
            _ => String::new(),
        };
        if !allowed.contains(&ext) {
            return Err(LpathError::ExtensionNotAllowed(ext));
        }
    }
    Ok(components.iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exts() -> Vec<String> {
        ["azw3", "mobi", "pdf", "txt"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn accepts_calibre_style_paths() {
        let e = exts();
        assert_eq!(
            validate("Martha Wells/Rogue Protocol - Martha Wells.azw3", Some(&e)).unwrap(),
            PathBuf::from("Martha Wells").join("Rogue Protocol - Martha Wells.azw3")
        );
        assert!(validate("Book.AZW3", Some(&e)).is_ok());
        assert!(validate("Ünïcödé/Tïtle.pdf", Some(&e)).is_ok());
    }

    #[test]
    fn rejects_escapes_and_oddities() {
        let e = exts();
        let cases = [
            ("", LpathError::Empty),
            ("/etc/passwd.txt", LpathError::Absolute),
            ("C:/x.txt", LpathError::Absolute),
            ("a\\b.txt", LpathError::BadCharacter('\\')),
            ("a\0b.txt", LpathError::BadCharacter('\0')),
            ("a\nb.txt", LpathError::BadCharacter('\n')),
            ("../x.txt", LpathError::BadComponent("..".into())),
            ("a/../../x.txt", LpathError::BadComponent("..".into())),
            ("a/./x.txt", LpathError::BadComponent(".".into())),
            ("a//x.txt", LpathError::BadComponent("".into())),
            ("a/", LpathError::BadComponent("".into())),
            (
                ".kws-1.part",
                LpathError::BadComponent(".kws-1.part".into()),
            ),
            ("x.sdr/y.txt", LpathError::BadComponent("y.txt".into())),
            ("book.epub", LpathError::ExtensionNotAllowed("epub".into())),
            ("noext", LpathError::ExtensionNotAllowed("".into())),
        ];
        for (input, want) in cases {
            assert_eq!(validate(input, Some(&e)), Err(want), "{input:?}");
        }
        assert_eq!(
            validate(&"a".repeat(1100), Some(&e)),
            Err(LpathError::TooLong)
        );
        let long = format!("{}.txt", "a".repeat(300));
        assert!(matches!(
            validate(&long, Some(&e)),
            Err(LpathError::BadComponent(_))
        ));
    }

    #[test]
    fn extension_check_is_optional() {
        assert!(validate("book.epub", None).is_ok());
        assert!(validate("../book.epub", None).is_err());
    }
}
