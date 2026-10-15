//! The device-side book store: the books directory plus our own state.
//!
//! Book files are written so the Kindle's scanner only ever sees complete
//! files (ADR 0005): new books stream into a dot-prefixed temporary file that
//! is renamed into place; replacements are copied over the existing file in
//! place, because the scanner ignores a rename over an indexed file.
//!
//! Our state (`books.json`, `driveinfo.json`) lives in a separate state
//! directory and holds Calibre's metadata for each book we received, keyed by
//! `lpath`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::lpath;

const BOOKS_FILE: &str = "books.json";
const DRIVEINFO_FILE: &str = "driveinfo.json";
const TEMP_PREFIX: &str = ".kws-";
const TEMP_SUFFIX: &str = ".part";
/// Space to leave free on top of an incoming book.
const FREE_SPACE_MARGIN: u64 = 1024 * 1024;

pub struct Library {
    books_dir: PathBuf,
    state_dir: PathBuf,
    books: Vec<Value>,
    driveinfo: Value,
    temp_counter: u32,
}

/// A book being received. Dropping it without committing removes the
/// temporary file.
pub struct Incoming {
    rel: PathBuf,
    temp_path: PathBuf,
    final_path: PathBuf,
    file: Option<File>,
}

impl Incoming {
    pub fn writer(&mut self) -> &mut dyn Write {
        self.file.as_mut().expect("incoming book already committed")
    }
}

impl Drop for Incoming {
    fn drop(&mut self) {
        if self.file.take().is_some() {
            let _ = fs::remove_file(&self.temp_path);
        }
    }
}

fn other(msg: impl Into<String>) -> io::Error {
    io::Error::other(msg.into())
}

impl Library {
    pub fn open(books_dir: &Path, state_dir: &Path) -> io::Result<Library> {
        fs::create_dir_all(books_dir)?;
        fs::create_dir_all(state_dir)?;
        let mut lib = Library {
            books_dir: books_dir.canonicalize()?,
            state_dir: state_dir.to_path_buf(),
            books: load_json(&state_dir.join(BOOKS_FILE))?
                .and_then(|v| match v {
                    Value::Array(a) => Some(a),
                    _ => None,
                })
                .unwrap_or_default(),
            driveinfo: load_json(&state_dir.join(DRIVEINFO_FILE))?.unwrap_or_else(|| json!({})),
            temp_counter: 0,
        };
        remove_stale_temps(&lib.books_dir, 4);
        let before = lib.books.len();
        let books_dir = lib.books_dir.clone();
        lib.books.retain(|b| {
            let keep = record_lpath(b)
                .and_then(|l| lpath::validate(l, None).ok())
                .is_some_and(|rel| books_dir.join(rel).is_file());
            if !keep {
                log::info!("forgetting missing book {:?}", b.get("lpath"));
            }
            keep
        });
        if lib.books.len() != before {
            lib.save()?;
        }
        Ok(lib)
    }

    pub fn books_dir(&self) -> &Path {
        &self.books_dir
    }

    pub fn books(&self) -> &[Value] {
        &self.books
    }

    pub fn book(&self, lpath: &str) -> Option<&Value> {
        self.books.iter().find(|b| record_lpath(b) == Some(lpath))
    }

    pub fn driveinfo(&self) -> &Value {
        &self.driveinfo
    }

    pub fn set_driveinfo(&mut self, driveinfo: Value) -> io::Result<()> {
        self.driveinfo = driveinfo;
        write_json_atomic(&self.state_dir.join(DRIVEINFO_FILE), &self.driveinfo)
    }

    /// Replaces the stored metadata of a book we hold. Returns false if the
    /// lpath is not one of ours.
    pub fn update_metadata(&mut self, metadata: Value) -> bool {
        let Some(l) = record_lpath(&metadata).map(str::to_owned) else {
            return false;
        };
        match self
            .books
            .iter_mut()
            .find(|b| record_lpath(b) == Some(l.as_str()))
        {
            Some(slot) => {
                *slot = slim(metadata, &l);
                true
            }
            None => false,
        }
    }

    pub fn save(&self) -> io::Result<()> {
        write_json_atomic(
            &self.state_dir.join(BOOKS_FILE),
            &Value::Array(self.books.clone()),
        )
    }

    pub fn free_space(&self) -> io::Result<u64> {
        statvfs(&self.books_dir).map(|(_, free)| free)
    }

    pub fn total_space(&self) -> io::Result<u64> {
        statvfs(&self.books_dir).map(|(total, _)| total)
    }

    /// Prepares to receive a book of `length` bytes at the already validated
    /// relative path `rel`.
    pub fn begin_incoming(&mut self, rel: &Path, length: u64) -> io::Result<Incoming> {
        let final_path = self.books_dir.join(rel);
        let parent = final_path
            .parent()
            .ok_or_else(|| other("book path has no parent"))?;
        fs::create_dir_all(parent)?;
        // A symlink inside the books directory must not redirect writes.
        if !parent.canonicalize()?.starts_with(&self.books_dir) {
            return Err(other("book path leaves the books directory"));
        }
        if final_path.is_dir() {
            return Err(other("a directory exists at the book path"));
        }
        let free = statvfs(parent)?.1;
        if free < length.saturating_add(FREE_SPACE_MARGIN) {
            return Err(other(format!(
                "not enough free space ({free} bytes free, book is {length})"
            )));
        }

        self.temp_counter += 1;
        let temp_path = parent.join(format!(
            "{TEMP_PREFIX}{}-{}{TEMP_SUFFIX}",
            std::process::id(),
            self.temp_counter
        ));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        Ok(Incoming {
            rel: rel.to_path_buf(),
            temp_path,
            final_path,
            file: Some(file),
        })
    }

    /// Moves a fully received book into place and records its metadata.
    pub fn commit_incoming(&mut self, mut incoming: Incoming, metadata: Value) -> io::Result<()> {
        let file = incoming
            .file
            .take()
            .expect("incoming book already committed");
        file.sync_all()?;
        drop(file);

        let result = if incoming.final_path.exists() {
            // Rewrite in place so the scanner updates the existing entry and
            // the reading position is kept (ADR 0005).
            replace_in_place(&incoming.temp_path, &incoming.final_path)
        } else {
            fs::rename(&incoming.temp_path, &incoming.final_path)
                .and_then(|_| sync_dir(&incoming.final_path))
        };
        let _ = fs::remove_file(&incoming.temp_path);
        result?;

        let l = rel_to_lpath(&incoming.rel);
        let record = slim(metadata, &l);
        match self
            .books
            .iter_mut()
            .find(|b| record_lpath(b) == Some(l.as_str()))
        {
            Some(slot) => *slot = record,
            None => self.books.push(record),
        }
        self.save()
    }

    /// Deletes a book we hold, with its firmware sidecar directory. Returns
    /// the book's Calibre UUID, or `None` if the lpath is not one of ours (in
    /// which case nothing is touched).
    pub fn delete_book(&mut self, lpath: &str) -> io::Result<Option<String>> {
        let Some(pos) = self
            .books
            .iter()
            .position(|b| record_lpath(b) == Some(lpath))
        else {
            return Ok(None);
        };
        let rel = lpath::validate(lpath, None).map_err(|e| other(e.to_string()))?;
        let path = self.books_dir.join(&rel);
        remove_if_exists(fs::remove_file(&path))?;
        remove_if_exists(fs::remove_dir_all(path.with_extension("sdr")))?;
        // Calibre's templates create per-author folders; drop them once empty.
        let mut dir = path.parent();
        while let Some(d) = dir {
            if d == self.books_dir || fs::remove_dir(d).is_err() {
                break;
            }
            dir = d.parent();
        }

        let record = self.books.remove(pos);
        self.save()?;
        Ok(Some(
            record
                .get("uuid")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ))
    }

    /// Opens a book we hold for sending back to Calibre.
    pub fn open_book(&self, lpath: &str) -> io::Result<(File, u64)> {
        if self.book(lpath).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "not a book we hold",
            ));
        }
        let rel = lpath::validate(lpath, None).map_err(|e| other(e.to_string()))?;
        let file = File::open(self.books_dir.join(rel))?;
        let len = file.metadata()?.len();
        Ok((file, len))
    }
}

fn record_lpath(record: &Value) -> Option<&str> {
    record.get("lpath").and_then(Value::as_str)
}

fn rel_to_lpath(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Our stored copy of Calibre's metadata: the lpath pinned to the one we
/// validated, and without the cover thumbnail (ADR 0006).
fn slim(mut metadata: Value, lpath: &str) -> Value {
    if !metadata.is_object() {
        metadata = json!({});
    }
    metadata["lpath"] = json!(lpath);
    metadata["thumbnail"] = Value::Null;
    metadata
}

fn replace_in_place(src: &Path, dst: &Path) -> io::Result<()> {
    let mut input = File::open(src)?;
    let mut output = OpenOptions::new().write(true).truncate(true).open(dst)?;
    io::copy(&mut input, &mut output)?;
    output.sync_all()
}

fn sync_dir(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(dir) => File::open(dir)?.sync_all(),
        None => Ok(()),
    }
}

fn remove_if_exists(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn load_json(path: &Path) -> io::Result<Option<Value>> {
    let data = match fs::read(path) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    match serde_json::from_slice(&data) {
        Ok(v) => Ok(Some(v)),
        Err(e) => {
            let bad = path.with_extension("json.bad");
            log::warn!(
                "{} is corrupt ({e}); moved to {}",
                path.display(),
                bad.display()
            );
            fs::rename(path, bad)?;
            Ok(None)
        }
    }
}

fn write_json_atomic(path: &Path, value: &Value) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = File::create(&tmp)?;
        serde_json::to_writer(&mut f, value).map_err(io::Error::from)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    sync_dir(path)
}

/// Removes temporary files left by an interrupted transfer.
fn remove_stale_temps(dir: &Path, depth: u32) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_file() && name.starts_with(TEMP_PREFIX) && name.ends_with(TEMP_SUFFIX) {
            log::info!("removing stale temporary file {}", entry.path().display());
            let _ = fs::remove_file(entry.path());
        } else if kind.is_dir() && depth > 0 && !name.ends_with(".sdr") {
            remove_stale_temps(&entry.path(), depth - 1);
        }
    }
}

/// Returns (total, available) bytes of the filesystem holding `path`.
fn statvfs(path: &Path) -> io::Result<(u64, u64)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path =
        CString::new(path.as_os_str().as_bytes()).map_err(|_| other("path contains NUL"))?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: c_path is a valid NUL-terminated string and st a valid out-pointer.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let frsize = st.f_frsize as u64;
    #[allow(clippy::unnecessary_cast)]
    Ok((st.f_blocks as u64 * frsize, st.f_bavail as u64 * frsize))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Library) {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(&tmp.path().join("books"), &tmp.path().join("state")).unwrap();
        (tmp, lib)
    }

    fn receive(lib: &mut Library, l: &str, data: &[u8], meta: Value) {
        let rel = lpath::validate(l, None).unwrap();
        let mut inc = lib.begin_incoming(&rel, data.len() as u64).unwrap();
        inc.writer().write_all(data).unwrap();
        lib.commit_incoming(inc, meta).unwrap();
    }

    #[test]
    fn receives_new_book_without_leaving_temps() {
        let (tmp, mut lib) = setup();
        receive(
            &mut lib,
            "Author/Title.azw3",
            b"book",
            json!({"uuid": "u1", "title": "T", "thumbnail": [1, 2, "x"]}),
        );
        let path = lib.books_dir().join("Author/Title.azw3");
        assert_eq!(fs::read(&path).unwrap(), b"book");
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec!["Title.azw3"]);
        let rec = lib.book("Author/Title.azw3").unwrap();
        assert_eq!(rec["uuid"], "u1");
        assert_eq!(rec["thumbnail"], Value::Null);

        // State survives a reopen.
        let lib2 = Library::open(&tmp.path().join("books"), &tmp.path().join("state")).unwrap();
        assert_eq!(lib2.books().len(), 1);
    }

    #[test]
    fn replacement_is_written_in_place() {
        let (_tmp, mut lib) = setup();
        receive(
            &mut lib,
            "Title.azw3",
            b"version one",
            json!({"uuid": "u1"}),
        );
        let path = lib.books_dir().join("Title.azw3");
        let ino = std::os::unix::fs::MetadataExt::ino(&fs::metadata(&path).unwrap());
        receive(
            &mut lib,
            "Title.azw3",
            b"v2",
            json!({"uuid": "u1", "title": "new"}),
        );
        assert_eq!(fs::read(&path).unwrap(), b"v2");
        assert_eq!(
            std::os::unix::fs::MetadataExt::ino(&fs::metadata(&path).unwrap()),
            ino
        );
        assert_eq!(lib.books().len(), 1);
        assert_eq!(lib.book("Title.azw3").unwrap()["title"], "new");
    }

    #[test]
    fn dropped_incoming_removes_temp() {
        let (_tmp, mut lib) = setup();
        let rel = lpath::validate("x.txt", None).unwrap();
        let mut inc = lib.begin_incoming(&rel, 3).unwrap();
        inc.writer().write_all(b"par").unwrap();
        drop(inc);
        assert_eq!(fs::read_dir(lib.books_dir()).unwrap().count(), 0);
        assert!(lib.book("x.txt").is_none());
    }

    #[test]
    fn stale_temps_and_missing_books_are_cleaned_on_open() {
        let (tmp, mut lib) = setup();
        receive(&mut lib, "gone.txt", b"x", json!({}));
        fs::remove_file(lib.books_dir().join("gone.txt")).unwrap();
        fs::create_dir_all(lib.books_dir().join("A")).unwrap();
        fs::write(lib.books_dir().join("A/.kws-1-1.part"), b"partial").unwrap();
        fs::write(lib.books_dir().join("A/keep.txt"), b"user file").unwrap();
        let lib = Library::open(&tmp.path().join("books"), &tmp.path().join("state")).unwrap();
        assert!(lib.books().is_empty());
        assert!(!lib.books_dir().join("A/.kws-1-1.part").exists());
        assert!(lib.books_dir().join("A/keep.txt").exists());
    }

    #[test]
    fn delete_removes_book_sidecar_and_empty_dirs_only_for_tracked_books() {
        let (_tmp, mut lib) = setup();
        receive(
            &mut lib,
            "Author/Title.azw3",
            b"book",
            json!({"uuid": "u1"}),
        );
        let sdr = lib.books_dir().join("Author/Title.sdr");
        fs::create_dir_all(&sdr).unwrap();
        fs::write(sdr.join("Title1234.azw3f"), b"pos").unwrap();
        fs::write(lib.books_dir().join("untracked.txt"), b"mine").unwrap();

        assert_eq!(lib.delete_book("untracked.txt").unwrap(), None);
        assert!(lib.books_dir().join("untracked.txt").exists());

        assert_eq!(
            lib.delete_book("Author/Title.azw3").unwrap(),
            Some("u1".into())
        );
        assert!(!lib.books_dir().join("Author").exists());
        assert!(lib.books().is_empty());
    }

    #[test]
    fn refuses_writes_through_symlinks_out_of_the_books_dir() {
        let (tmp, mut lib) = setup();
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, lib.books_dir().join("link")).unwrap();
        let rel = lpath::validate("link/evil.txt", None).unwrap();
        assert!(lib.begin_incoming(&rel, 1).is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    }

    #[test]
    fn refuses_books_larger_than_free_space() {
        let (_tmp, mut lib) = setup();
        let rel = lpath::validate("huge.pdf", None).unwrap();
        let err = lib.begin_incoming(&rel, u64::MAX / 2).err().unwrap();
        assert!(err.to_string().contains("not enough free space"), "{err}");
    }

    #[test]
    fn update_metadata_only_touches_held_books() {
        let (_tmp, mut lib) = setup();
        receive(&mut lib, "a.txt", b"x", json!({"title": "old"}));
        assert!(lib
            .update_metadata(json!({"lpath": "a.txt", "title": "new", "thumbnail": [1, 1, "x"]})));
        assert_eq!(lib.book("a.txt").unwrap()["title"], "new");
        assert_eq!(lib.book("a.txt").unwrap()["thumbnail"], Value::Null);
        assert!(!lib.update_metadata(json!({"lpath": "b.txt"})));
    }
}
