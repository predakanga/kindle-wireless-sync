// SPDX-License-Identifier: GPL-3.0-or-later
//! The folder picker for where new books go (ADR 0023): folders inside the
//! books folder, which is as far up as the picker goes. Paths are relative
//! to it, `/`-separated, and empty for the books folder itself.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::lpath;

/// A folder in a listing, and why it can't be used, if it can't.
#[derive(Debug, Clone, PartialEq)]
pub struct Folder {
    pub name: String,
    pub problem: Option<String>,
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.into())
}

/// `name` inside the folder `rel`.
pub fn join(rel: &str, name: &str) -> String {
    match rel.trim_matches('/') {
        "" => name.to_owned(),
        rel => format!("{rel}/{name}"),
    }
}

/// The folder `rel` inside `root`, checked: a valid lpath (ADR 0003), an
/// existing folder, and still inside `root` once symlinks are followed.
pub fn resolve(root: &Path, rel: &str) -> io::Result<PathBuf> {
    let rel = rel.trim_matches('/');
    let dir = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(lpath::validate(rel, None).map_err(|e| invalid(format!("{rel}: {e}")))?)
    };
    let real = dir.canonicalize()?;
    if !real.starts_with(root.canonicalize()?) {
        return Err(invalid(format!("{rel} leads outside the books folder")));
    }
    if !real.is_dir() {
        return Err(invalid(format!("{rel} isn't a folder")));
    }
    Ok(dir)
}

/// The folders inside `rel`, by name, leaving out hidden folders and the
/// Kindle's `.sdr` folders.
pub fn list(root: &Path, rel: &str) -> io::Result<Vec<Folder>> {
    let dir = resolve(root, rel)?;
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.to_ascii_lowercase().ends_with(".sdr") {
            continue;
        }
        // Follows symlinks; `resolve` checks where one leads.
        if !dir.join(&name).is_dir() {
            continue;
        }
        let problem = resolve(root, &join(rel, &name))
            .err()
            .map(|e| e.to_string());
        out.push(Folder { name, problem });
    }
    out.sort_by_key(|f| f.name.to_lowercase());
    Ok(out)
}

/// Creates the folder `name` inside `rel`, and returns its path.
pub fn create(root: &Path, rel: &str, name: &str) -> io::Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("Enter a name for the folder."));
    }
    if name.contains('/') {
        return Err(invalid("A folder name can't contain \"/\"."));
    }
    let parent = resolve(root, rel)?;
    let path = join(rel, name);
    lpath::validate(&path, None).map_err(|e| invalid(format!("{name}: {e}")))?;
    match fs::create_dir(parent.join(name)) {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            Err(invalid(format!("There's already a folder called {name}.")))
        }
        other => other.map(|()| path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for d in [
            "Calibre/SF",
            "Calibre/Fantasy",
            "comics",
            ".hidden",
            "Book.sdr",
        ] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        fs::write(root.join("Book.azw3"), b"x").unwrap();
        tmp
    }

    fn names(folders: &[Folder]) -> Vec<&str> {
        folders.iter().map(|f| f.name.as_str()).collect()
    }

    #[test]
    fn lists_folders_only_without_hidden_or_sdr() {
        let tmp = tree();
        let top = list(tmp.path(), "").unwrap();
        assert_eq!(names(&top), ["Calibre", "comics"]);
        assert!(top.iter().all(|f| f.problem.is_none()));
        assert_eq!(
            names(&list(tmp.path(), "Calibre").unwrap()),
            ["Fantasy", "SF"]
        );
        assert_eq!(
            names(&list(tmp.path(), "/Calibre/").unwrap()),
            ["Fantasy", "SF"]
        );
    }

    #[test]
    fn stays_inside_the_books_folder() {
        let tmp = tree();
        for rel in [
            "..",
            "Calibre/..",
            "/etc",
            ".hidden",
            "Book.sdr",
            "missing",
            "Book.azw3",
        ] {
            assert!(resolve(tmp.path(), rel).is_err(), "{rel}");
        }
        // A link out of the books folder is listed, but can't be used.
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("Elsewhere")).unwrap();
        let top = list(tmp.path(), "").unwrap();
        let link = top.iter().find(|f| f.name == "Elsewhere").unwrap();
        assert!(link.problem.as_deref().unwrap().contains("outside"));
        assert!(list(tmp.path(), "Elsewhere").is_err());
    }

    #[test]
    fn creates_folders_with_valid_names_only() {
        let tmp = tree();
        assert_eq!(
            create(tmp.path(), "Calibre", " Horror ").unwrap(),
            "Calibre/Horror"
        );
        assert!(tmp.path().join("Calibre/Horror").is_dir());
        for bad in ["", "  ", "a/b", "..", ".x", "x.sdr", "Horror"] {
            assert!(create(tmp.path(), "Calibre", bad).is_err(), "{bad:?}");
        }
        assert!(create(tmp.path(), "..", "x").is_err());
    }
}
