// SPDX-License-Identifier: GPL-3.0-or-later
//! The device-side book store: the books directory plus our own state.
//!
//! Book files are written so the Kindle's scanner only ever sees complete
//! files (ADR 0005): new books stream into a dot-prefixed temporary file that
//! is renamed into place; replacements are copied over the existing file in
//! place, because the scanner ignores a rename over an indexed file.
//!
//! Our state (`books.json`, `driveinfo.json`) lives in a
//! separate state directory and holds Calibre's metadata for each book we received, keyed by
//! `lpath`. Each record also carries our private `_kws` object (the book's
//! Kindle content key), which is stripped before metadata goes to Calibre.
//!
//! With covers enabled, Calibre's thumbnail is written to the firmware's
//! thumbnails directory before the book appears, and backed up so it can be
//! restored if the firmware replaces it (see [`crate::kindle`]).

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Map, Value};

use crate::adopt;
use crate::catalog::Catalog;
use crate::kindle::{self, BookKey, CoverConfig, KeySource};
use crate::lpath;
use crate::readsync::{self, Columns};
use crate::series;

const BOOKS_FILE: &str = "books.json";
const DRIVEINFO_FILE: &str = "driveinfo.json";
/// Where libraries' columns were kept before they moved into `config.json`
/// (ADR 0020). Removed when found.
const OLD_COLUMNS_FILE: &str = "columns.json";
const TEMP_PREFIX: &str = ".kws-";
const TEMP_SUFFIX: &str = ".part";
/// Space to leave free on top of an incoming book.
const FREE_SPACE_MARGIN: u64 = 1024 * 1024;
/// Our private key in each book record.
const PRIVATE_KEY: &str = "_kws";
const COVERS_DIR: &str = "covers";
/// How long to wait for the scanner when a key is only known after indexing.
const INDEX_WAIT: Duration = Duration::from_secs(10);

pub struct Library {
    books_dir: PathBuf,
    state_dir: PathBuf,
    books: Vec<Value>,
    driveinfo: Value,
    temp_counter: u32,
    covers: Option<CoverConfig>,
    adopt_memo: adopt::Memo,
    series: Option<SeriesSync>,
    /// The Kindle's catalog, for reading read states (ADR 0017).
    catalog: Option<Catalog>,
}

/// How long to wait after the last book change before updating series
/// (ADR 0016): long enough for the scanner to index new files and for the
/// firmware's own series processing after a delete, which we must follow.
pub const SERIES_DEBOUNCE: Duration = Duration::from_secs(15);

/// Keeps the catalog's series in line with our books (ADR 0016).
struct SeriesSync {
    catalog: Catalog,
    debounce: Duration,
    /// False when series are turned off: passes dissolve ours instead.
    keep: bool,
    /// Set by any book change; cleared by a complete pass.
    dirty_since: Option<Instant>,
    /// We've logged that the catalog isn't ready yet.
    waiting: bool,
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
            covers: None,
            adopt_memo: adopt::Memo::default(),
            series: None,
            catalog: None,
        };
        if let Err(e) = remove_if_exists(fs::remove_file(state_dir.join(OLD_COLUMNS_FILE))) {
            log::warn!("cannot remove {OLD_COLUMNS_FILE}: {e}");
        }
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

    /// Turns on cover handling and restores any covers the firmware lost.
    pub fn enable_covers(&mut self, covers: CoverConfig) -> io::Result<()> {
        fs::create_dir_all(self.state_dir.join(COVERS_DIR))?;
        self.covers = Some(covers);
        let restored = self.restore_covers();
        if restored > 0 {
            log::info!("restored {restored} cover thumbnails");
        }
        Ok(())
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

    /// A book's metadata as Calibre should see it (without our private data).
    pub fn calibre_metadata(&self, index: usize) -> Option<Value> {
        let mut record = self.books.get(index)?.clone();
        if let Some(obj) = record.as_object_mut() {
            obj.remove(PRIVATE_KEY);
        }
        Some(record)
    }

    pub fn driveinfo(&self) -> &Value {
        &self.driveinfo
    }

    pub fn set_driveinfo(&mut self, driveinfo: Value) -> io::Result<()> {
        self.driveinfo = driveinfo;
        write_json_atomic(&self.state_dir.join(DRIVEINFO_FILE), &self.driveinfo)
    }

    /// Lets read-status sync read the Kindle's catalog (ADR 0017). Without
    /// it, we report Calibre's own values back.
    pub fn set_catalog(&mut self, catalog: Catalog) {
        self.catalog = Some(catalog);
    }

    /// The read-status fields to add to each book in the book list, in the
    /// order of [`Library::books`], for the columns named this session.
    /// Saves what we reported.
    pub fn read_sync_fields(&mut self, named: &Columns) -> io::Result<Vec<Map<String, Value>>> {
        if named.is_empty() {
            return Ok(vec![Map::new(); self.books.len()]);
        }
        let states = match &self.catalog {
            Some(c) => match readsync::kindle_states(c, &self.books_dir) {
                Ok(s) => s,
                Err(e) => {
                    // Calibre still gets its own values back, so nothing is
                    // lost; Kindle changes wait for the next connection.
                    log::warn!("cannot read the Kindle's read states: {e}");
                    Default::default()
                }
            },
            None => Default::default(),
        };
        let books_dir = self.books_dir.clone();
        let fields = self
            .books
            .iter_mut()
            .map(|record| {
                let location =
                    record_lpath(record).map(|l| books_dir.join(l).to_string_lossy().into_owned());
                let kindle = location.and_then(|l| states.get(&l));
                readsync::report(record, named, kindle)
            })
            .collect();
        self.save()?;
        Ok(fields)
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
                let private = slot.get(PRIVATE_KEY).cloned();
                // Calibre resends metadata on every connect; only a change
                // of series needs a series pass.
                let series_changed = slot["series"] != metadata["series"]
                    || slot["series_index"] != metadata["series_index"];
                *slot = slim(metadata, &l);
                if let Some(private) = private {
                    slot[PRIVATE_KEY] = private;
                }
                if series_changed {
                    self.mark_series_dirty();
                }
                true
            }
            None => false,
        }
    }

    /// Turns on series (ADR 0016), and asks for a first pass straight away.
    pub fn enable_series(&mut self, catalog: Catalog, debounce: Duration) {
        self.start_series(catalog, debounce, true);
    }

    /// Series turned off: a pass dissolves the series we created, once the
    /// catalog is ready.
    pub fn dissolve_series(&mut self, catalog: Catalog, debounce: Duration) {
        self.start_series(catalog, debounce, false);
    }

    fn start_series(&mut self, catalog: Catalog, debounce: Duration, keep: bool) {
        self.series = Some(SeriesSync {
            catalog,
            debounce,
            keep,
            dirty_since: Some(Instant::now() - debounce),
            waiting: false,
        });
    }

    fn mark_series_dirty(&mut self) {
        if let Some(s) = &mut self.series {
            s.dirty_since = Some(Instant::now());
        }
    }

    /// How long until a series pass is due, if one is needed.
    pub fn series_due_in(&self) -> Option<Duration> {
        let s = self.series.as_ref()?;
        Some(s.debounce.saturating_sub(s.dirty_since?.elapsed()))
    }

    /// Runs a series pass if one is due. Errors are logged, and the pass is
    /// retried after the next debounce.
    pub fn reconcile_series_if_due(&mut self) {
        if self.series_due_in() == Some(Duration::ZERO) {
            self.reconcile_series();
        }
    }

    /// Brings the catalog's series in line with our books now.
    pub fn reconcile_series(&mut self) {
        let Some(s) = &mut self.series else { return };
        if !s.catalog.ready() {
            if !s.waiting {
                log::info!("series: waiting for the Kindle's catalog");
                s.waiting = true;
            }
            s.dirty_since = Some(Instant::now());
            return;
        }
        s.waiting = false;
        let books = match s.keep {
            true => series::books_from_records(&self.books),
            false => Vec::new(),
        };
        match apply_series(&s.catalog, &self.books_dir, &books) {
            Ok((n, pending)) => {
                if n > 0 {
                    log::info!("series: applied {n} catalog changes");
                }
                // A member not yet indexed: try again after the debounce.
                s.dirty_since = pending.then(Instant::now);
            }
            Err(e) => {
                log::warn!("series: {e}; will retry");
                s.dirty_since = Some(Instant::now());
            }
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

        // The cover must exist before the scanner first sees the book: the
        // Library shows whatever thumbnail is there at first draw, and the
        // scanner never replaces an existing one.
        let ext = incoming
            .rel
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut key = None;
        let mut key_after_index = false;
        if self.covers.is_some() {
            match kindle::key_source(&ext, &read_head(&incoming.temp_path)?) {
                KeySource::Known(k) => {
                    self.write_cover(&k, &metadata);
                    key = Some(k);
                }
                KeySource::AfterIndex => key_after_index = true,
                KeySource::NoCover => {}
            }
        }

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

        if key_after_index {
            key = self.wait_for_indexed_key(&incoming.final_path);
            match &key {
                Some(k) => self.write_cover(k, &metadata),
                None => log::warn!(
                    "no content key for {}; cover not set",
                    incoming.rel.display()
                ),
            }
        }

        let l = rel_to_lpath(&incoming.rel);
        let mut record = slim(metadata, &l);
        if let Some(k) = key {
            record[PRIVATE_KEY] = json!({"cde_key": k.cde_key, "cde_type": k.cde_type});
        }
        match self
            .books
            .iter_mut()
            .find(|b| record_lpath(b) == Some(l.as_str()))
        {
            Some(slot) => *slot = record,
            None => self.books.push(record),
        }
        self.mark_series_dirty();
        self.save()
    }

    fn wait_for_indexed_key(&self, path: &Path) -> Option<BookKey> {
        let db = self.covers.as_ref()?.catalog_db.as_ref()?;
        let location = path.to_str()?;
        let deadline = Instant::now() + INDEX_WAIT;
        loop {
            if let Some(k) = kindle::lookup_indexed_key(db, location) {
                return Some(k);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// Writes Calibre's thumbnail for `key` and its backup. Failures are
    /// logged: a missing cover is not worth failing the transfer for.
    fn write_cover(&self, key: &BookKey, metadata: &Value) {
        let Some(covers) = &self.covers else { return };
        let Some(jpeg) = metadata["thumbnail"][2]
            .as_str()
            .and_then(kindle::decode_base64)
        else {
            log::info!("no usable cover thumbnail from calibre for {}", key.cde_key);
            return;
        };
        let name = key.thumbnail_name();
        let result = write_file_atomic(&self.state_dir.join(COVERS_DIR).join(&name), &jpeg)
            .and_then(|_| write_file_atomic(&covers.thumbnails_dir.join(&name), &jpeg));
        if let Err(e) = result {
            log::warn!("failed to write cover {name}: {e}");
        }
    }

    /// Puts back covers the firmware deleted or replaced. Returns how many
    /// were restored.
    pub fn restore_covers(&self) -> usize {
        let Some(covers) = &self.covers else { return 0 };
        let mut restored = 0;
        for key in self.books.iter().filter_map(record_key) {
            let name = key.thumbnail_name();
            let backup = self.state_dir.join(COVERS_DIR).join(&name);
            let target = covers.thumbnails_dir.join(&name);
            let (Ok(b), current) = (fs::metadata(&backup), fs::metadata(&target)) else {
                continue;
            };
            if current.map(|c| c.len()).ok() == Some(b.len()) {
                continue;
            }
            match fs::read(&backup).and_then(|data| write_file_atomic(&target, &data)) {
                Ok(()) => restored += 1,
                Err(e) => log::warn!("failed to restore cover {name}: {e}"),
            }
        }
        restored
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
        if let (Some(covers), Some(key)) = (&self.covers, record_key(&record)) {
            let name = key.thumbnail_name();
            remove_if_exists(fs::remove_file(covers.thumbnails_dir.join(&name)))?;
            remove_if_exists(fs::remove_file(self.state_dir.join(COVERS_DIR).join(&name)))?;
        }
        self.mark_series_dirty();
        self.save()?;
        Ok(Some(
            record
                .get("uuid")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ))
    }

    /// Records book files that Calibre's USB driver sent, so Calibre's
    /// wireless connection sees them (ADR 0013). Only files with a Calibre
    /// UUID are adopted; nothing on disk changes. Returns how many were
    /// adopted.
    pub fn adopt_existing(&mut self, accepted: &[String]) -> io::Result<usize> {
        let known: HashSet<String> = self
            .books
            .iter()
            .filter_map(record_lpath)
            .map(str::to_owned)
            .collect();
        let candidates = adopt::find_unknown(&self.books_dir, &known, accepted, &self.adopt_memo);
        if candidates.is_empty() {
            return Ok(0);
        }
        // metadata.calibre sits at the storage root, next to the books
        // directory, and names books relative to that root.
        let usb = match (self.books_dir.parent(), self.books_dir.file_name()) {
            (Some(root), Some(name)) => adopt::read_usb_metadata(
                &root.join(adopt::USB_METADATA_FILE),
                &format!("{}/", name.to_string_lossy()),
            ),
            _ => Default::default(),
        };
        let mut adopted = 0;
        for c in candidates {
            let head = if adopt::is_mobi_family(&c) {
                read_head(&c.path).ok()
            } else {
                None
            };
            let Some((mut record, source)) = adopt::record_for(&c, &usb, head.as_deref()) else {
                self.adopt_memo.reject(&c);
                continue;
            };
            let key = head.as_deref().and_then(kindle::read_exth_key).or_else(|| {
                let db = self.covers.as_ref()?.catalog_db.as_ref()?;
                kindle::lookup_indexed_key(db, c.path.to_str()?)
            });
            if let Some(k) = key {
                self.back_up_cover(&k);
                record[PRIVATE_KEY] = json!({"cde_key": k.cde_key, "cde_type": k.cde_type});
            }
            log::info!("adopted {} ({source:?})", c.lpath);
            self.books.push(record);
            adopted += 1;
        }
        if adopted > 0 {
            self.mark_series_dirty();
            self.save()?;
        }
        Ok(adopted)
    }

    /// Keeps a copy of a cover someone else wrote (the USB driver does), so
    /// [`Library::restore_covers`] can put it back like ours.
    fn back_up_cover(&self, key: &BookKey) {
        let Some(covers) = &self.covers else { return };
        let name = key.thumbnail_name();
        let backup = self.state_dir.join(COVERS_DIR).join(&name);
        if backup.exists() {
            return;
        }
        if let Ok(jpeg) = fs::read(covers.thumbnails_dir.join(&name)) {
            if let Err(e) = write_file_atomic(&backup, &jpeg) {
                log::warn!("failed to back up cover {name}: {e}");
            }
        }
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

    /// Before a session with the linked library: if our state belongs to a
    /// different library, forget that library (ADR 0018). The library is the
    /// `last_library_uuid` Calibre stores in our driveinfo. Returns whether
    /// it reset.
    pub fn reset_if_relinked(&mut self, linked_uuid: &str) -> io::Result<bool> {
        if self.driveinfo.as_object().is_none_or(Map::is_empty) {
            return Ok(false);
        }
        match self.driveinfo["last_library_uuid"].as_str() {
            Some(u) if u == linked_uuid => Ok(false),
            Some(u) if !u.is_empty() => {
                log::info!("linked to a different library ({linked_uuid}, was {u}): resetting");
                self.forget_library()?;
                Ok(true)
            }
            _ => {
                log::warn!("driveinfo.json names no library; not resetting");
                Ok(false)
            }
        }
    }

    /// Forgets everything that came from the previous library: each book's
    /// metadata except its identity and series, read-sync state, and
    /// Calibre's device identity, so Calibre sees a new device with an
    /// empty metadata cache. The driveinfo goes last: an interrupted reset
    /// runs again.
    fn forget_library(&mut self) -> io::Result<()> {
        let books_dir = self.books_dir.clone();
        for record in &mut self.books {
            let modified = record_lpath(record)
                .and_then(|l| lpath::validate(l, None).ok())
                .and_then(|rel| fs::metadata(books_dir.join(rel)).ok())
                .and_then(|m| m.modified().ok());
            *record = identity(record, modified);
        }
        self.save()?;
        self.mark_series_dirty();
        remove_if_exists(fs::remove_file(self.state_dir.join(DRIVEINFO_FILE)))?;
        self.driveinfo = json!({});
        Ok(())
    }
}

/// The parts of a book record that aren't the library's (ADR 0018): what the
/// next library matches on, and our private data minus read-sync state.
/// Records built by adoption (ADR 0013) have the same shape, including
/// `last_modified` from the file: Calibre resends a book's metadata only when
/// its own `last_modified` differs, so the old library's date would stop
/// that library ever filling the record in again.
fn identity(record: &Value, file_modified: Option<SystemTime>) -> Value {
    // Series stay: they group the Kindle's library, which is useful even
    // before the new library resends a book's metadata, or if it never does.
    const KEPT: [&str; 8] = [
        "lpath",
        "size",
        "last_modified",
        "uuid",
        "title",
        "authors",
        "series",
        "series_index",
    ];
    let mut out = Map::new();
    for key in KEPT {
        if let Some(v) = record.get(key) {
            out.insert(key.into(), v.clone());
        }
    }
    if let Some(t) = file_modified {
        out.insert("last_modified".into(), json!(adopt::iso_utc(t)));
    }
    out.insert("thumbnail".into(), Value::Null);
    if let Some(mut private) = record.get(PRIVATE_KEY).cloned() {
        readsync::forget(&mut private);
        out.insert(PRIVATE_KEY.into(), private);
    }
    Value::Object(out)
}

fn record_lpath(record: &Value) -> Option<&str> {
    record.get("lpath").and_then(Value::as_str)
}

fn record_key(record: &Value) -> Option<BookKey> {
    let private = record.get(PRIVATE_KEY)?;
    Some(BookKey {
        cde_key: private.get("cde_key")?.as_str()?.to_owned(),
        cde_type: private.get("cde_type")?.as_str()?.to_owned(),
    })
}

/// The start of a book file, enough for [`kindle::key_source`].
fn read_head(path: &Path) -> io::Result<Vec<u8>> {
    let mut head = Vec::new();
    File::open(path)?
        .take(kindle::HEAD_LEN as u64)
        .read_to_end(&mut head)?;
    Ok(head)
}

/// Writes via a dot-prefixed temporary file and rename.
fn write_file_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let name = path.file_name().ok_or_else(|| other("no file name"))?;
    let tmp = path.with_file_name(format!("{TEMP_PREFIX}{}.tmp", name.to_string_lossy()));
    {
        let mut f = File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

fn rel_to_lpath(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// One series pass: returns how many changes were sent, and whether a
/// member wasn't indexed yet.
fn apply_series(
    catalog: &Catalog,
    books_dir: &Path,
    books: &[series::Book],
) -> io::Result<(usize, bool)> {
    let snap = series::snapshot(catalog, books_dir)?;
    let plan = series::plan(books, &snap);
    catalog.change(&plan.commands)?;
    Ok((plan.commands.len(), plan.pending))
}

/// Dissolves all of our series (uninstall, or series turned off).
pub fn remove_all_series(catalog: &Catalog, books_dir: &Path) -> io::Result<usize> {
    apply_series(catalog, books_dir, &[]).map(|(n, _)| n)
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

    /// A fake catalog: a real sqlite file with the columns series reads,
    /// and a change endpoint that records each request's commands.
    struct FakeCatalog {
        catalog: Catalog,
        requests: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
    }

    fn fake_catalog(dir: &Path, rows: &str) -> FakeCatalog {
        use std::io::{BufRead, BufReader};
        let db = dir.join("cc.db");
        let schema = "CREATE TABLE Entries (p_uuid, p_type, p_location, p_cdeKey, p_cdeType, \
            p_thumbnail, p_seriesState, p_originType, p_titles_0_nominal, p_memberCount, \
            p_homeMemberCount, p_isProcessed, p_virtualCollectionCount, j_members); \
            CREATE TABLE Series (d_seriesId, d_itemCdeKey, d_itemPosition, d_itemPositionLabel);";
        let ok = std::process::Command::new("sqlite3")
            .arg(&db)
            .arg(format!("{schema} {rows}"))
            .status()
            .unwrap();
        assert!(ok.success());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut s = stream.unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut len = 0;
                loop {
                    let mut line = String::new();
                    r.read_line(&mut line).unwrap();
                    if let Some(v) = line.strip_prefix("Content-Length: ") {
                        len = v.trim().parse().unwrap();
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                let v: Value = serde_json::from_slice(&body).unwrap();
                seen.lock().unwrap().push(v["commands"].clone());
                let reply = r#"{"ok":true,"type":"ChangeResponse","changes":1}"#;
                write!(
                    s,
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{reply}",
                    reply.len()
                )
                .unwrap();
            }
        });
        let token_file = dir.join("token");
        fs::write(&token_file, "t").unwrap();
        FakeCatalog {
            catalog: Catalog {
                db,
                endpoint,
                token_file,
            },
            requests,
        }
    }

    #[test]
    fn series_pass_runs_after_changes_and_only_then() {
        let tmp = tempfile::tempdir().unwrap();
        let books = tmp.path().join("books");
        fs::create_dir_all(&books).unwrap();
        let books = books.canonicalize().unwrap();
        let state = tmp.path().join("state");
        fs::create_dir_all(&state).unwrap();
        let mut records = Vec::new();
        for (i, k) in ["k1", "k2"].iter().enumerate() {
            fs::write(books.join(format!("{k}.azw3")), b"x").unwrap();
            records.push(json!({"lpath": format!("{k}.azw3"), "title": k, "authors": ["Jane Author"],
                "series": "Saga", "series_index": i + 1, "_kws": {"cde_key": k, "cde_type": "EBOK"}}));
        }
        fs::write(state.join(BOOKS_FILE), Value::Array(records).to_string()).unwrap();
        let rows = format!(
            "INSERT INTO Entries VALUES ('u1','Entry:Item','{b}/k1.azw3','k1','EBOK','/t/k1.jpg',1,NULL,'k1',0,0,0,0,NULL), \
             ('u2','Entry:Item','{b}/k2.azw3','k2','EBOK','/t/k2.jpg',1,NULL,'k2',0,0,0,0,NULL), \
             ('x','Entry:Item','/elsewhere/k9.azw3','k9','EBOK','',1,NULL,'k9',0,0,0,0,NULL);",
            b = books.display()
        );
        let fake = fake_catalog(tmp.path(), &rows);
        let mut lib = Library::open(&books, &state).unwrap();
        assert_eq!(lib.series_due_in(), None, "off until enabled");
        lib.enable_series(fake.catalog, Duration::ZERO);
        assert_eq!(
            lib.series_due_in(),
            Some(Duration::ZERO),
            "a first pass is due at once"
        );

        lib.reconcile_series_if_due();
        let sent = fake.requests.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        let cmds = sent[0].as_array().unwrap();
        assert_eq!(cmds.len(), 6, "{cmds:?}");
        assert_eq!(
            cmds[2]["insertOr"]["entry"]["cdeKey"],
            series::series_key("Saga")
        );
        assert_eq!(cmds[3]["update"]["members"], json!(["u1", "u2"]));
        assert_eq!(lib.series_due_in(), None, "a complete pass clears the flag");

        // Calibre resending the same metadata isn't a series change.
        let same = lib.books()[0].clone();
        assert!(lib.update_metadata(same.clone()));
        assert_eq!(lib.series_due_in(), None);
        let mut moved = same;
        moved["series_index"] = json!(3);
        assert!(lib.update_metadata(moved));
        assert_eq!(lib.series_due_in(), Some(Duration::ZERO));
    }

    #[test]
    fn read_sync_reads_the_kindle_by_path_and_remembers_reports() {
        let tmp = tempfile::tempdir().unwrap();
        let (books, state) = (tmp.path().join("books"), tmp.path().join("state"));
        fs::create_dir_all(&books).unwrap();
        fs::create_dir_all(&state).unwrap();
        let books = books.canonicalize().unwrap();
        let records: Vec<Value> = ["a", "b"]
            .iter()
            .map(|k| {
                fs::write(books.join(format!("{k}.azw3")), b"x").unwrap();
                json!({"lpath": format!("{k}.azw3"), "title": k,
                       "user_metadata": {"#kindle_read": {"#value#": false}}})
            })
            .collect();
        fs::write(state.join(BOOKS_FILE), Value::Array(records).to_string()).unwrap();
        let db = tmp.path().join("cc.db");
        let rows = format!(
            "CREATE TABLE Entries (p_type, p_location, p_readState, p_lastOpenTime); \
             INSERT INTO Entries VALUES ('Entry:Item', '{b}/a.azw3', 2, 86400), \
             ('Entry:Item', '{b}/b.azw3', 0, NULL);",
            b = books.display()
        );
        assert!(std::process::Command::new("sqlite3")
            .arg(&db)
            .arg(rows)
            .status()
            .unwrap()
            .success());
        let named = Columns {
            read: Some("#kindle_read".into()),
            last_read: None,
        };

        let mut lib = Library::open(&books, &state).unwrap();
        lib.set_catalog(Catalog::new(db.clone()));
        let fields = lib.read_sync_fields(&named).unwrap();
        assert_eq!(fields[0]["_is_read_"], true, "finished on the Kindle");
        assert_eq!(fields[1]["_is_read_"], false, "no Kindle value: Calibre's");
        assert!(fields.iter().all(|f| f.get("_sync_type_").is_none()));

        // Calibre didn't take it (its metadata still says false): after a
        // restart, it's reported again.
        let mut lib = Library::open(&books, &state).unwrap();
        lib.set_catalog(Catalog::new(db));
        let fields = lib.read_sync_fields(&named).unwrap();
        assert_eq!(fields[0]["_is_read_"], true);
        // Calibre takes it and resends the metadata; then the user unticks
        // it in Calibre, and Calibre's value comes back.
        let mut meta = lib.books()[0].clone();
        meta["user_metadata"]["#kindle_read"]["#value#"] = json!(true);
        assert!(lib.update_metadata(meta.clone()));
        assert_eq!(lib.read_sync_fields(&named).unwrap()[0]["_is_read_"], true);
        meta["user_metadata"]["#kindle_read"]["#value#"] = json!(false);
        assert!(lib.update_metadata(meta));
        assert_eq!(lib.read_sync_fields(&named).unwrap()[0]["_is_read_"], false);
        // And nothing at all when no columns are named.
        let none = lib.read_sync_fields(&Columns::default()).unwrap();
        assert!(none.iter().all(Map::is_empty));
    }

    #[test]
    fn series_pass_waits_for_the_catalog_and_can_dissolve() {
        let tmp = tempfile::tempdir().unwrap();
        let books = tmp.path().join("books");
        fs::create_dir_all(&books).unwrap();
        let books = books.canonicalize().unwrap();
        let state = tmp.path().join("state");
        fs::create_dir_all(&state).unwrap();
        let mut records = Vec::new();
        for (i, k) in ["k1", "k2"].iter().enumerate() {
            fs::write(books.join(format!("{k}.azw3")), b"x").unwrap();
            records.push(json!({"lpath": format!("{k}.azw3"), "title": k,
                "series": "Saga", "series_index": i + 1, "_kws": {"cde_key": k, "cde_type": "EBOK"}}));
        }
        fs::write(state.join(BOOKS_FILE), Value::Array(records).to_string()).unwrap();
        let rows = format!(
            "INSERT INTO Entries VALUES ('u1','Entry:Item','{b}/k1.azw3','k1','EBOK','',1,NULL,'k1',0,0,0,0,NULL), \
             ('u2','Entry:Item','{b}/k2.azw3','k2','EBOK','',1,NULL,'k2',0,0,0,0,NULL);",
            b = books.display()
        );
        let fake = fake_catalog(tmp.path(), &rows);
        let token = fake.catalog.token_file.clone();
        let saved = fs::read(&token).unwrap();
        fs::remove_file(&token).unwrap();

        // At boot the token can appear after we start: wait, don't give up.
        let mut lib = Library::open(&books, &state).unwrap();
        lib.enable_series(fake.catalog, Duration::ZERO);
        lib.reconcile_series_if_due();
        assert!(fake.requests.lock().unwrap().is_empty());
        assert_eq!(lib.series_due_in(), Some(Duration::ZERO), "still due");
        fs::write(&token, &saved).unwrap();
        lib.reconcile_series_if_due();
        assert_eq!(fake.requests.lock().unwrap().len(), 1);
        assert_eq!(lib.series_due_in(), None);

        // Series turned off: the same books plan no series at all, so the
        // pass removes what the catalog holds of ours (nothing here).
        let catalog = Catalog {
            db: tmp.path().join("cc.db"),
            endpoint: "127.0.0.1:9".into(),
            token_file: token,
        };
        lib.dissolve_series(catalog, Duration::ZERO);
        lib.reconcile_series_if_due();
        assert_eq!(lib.series_due_in(), None, "nothing to send, so no request");
    }

    fn receive(lib: &mut Library, l: &str, data: &[u8], meta: Value) {
        let rel = lpath::validate(l, None).unwrap();
        let mut inc = lib.begin_incoming(&rel, data.len() as u64).unwrap();
        inc.writer().write_all(data).unwrap();
        lib.commit_incoming(inc, meta).unwrap();
    }

    #[test]
    fn the_old_columns_file_is_removed() {
        let (tmp, lib) = setup();
        let state = tmp.path().join("state");
        drop(lib);
        fs::write(state.join(OLD_COLUMNS_FILE), b"{}").unwrap();
        Library::open(&tmp.path().join("books"), &state).unwrap();
        assert!(!state.join(OLD_COLUMNS_FILE).exists());
    }

    #[test]
    fn linking_a_different_library_keeps_only_book_identities_and_series() {
        let (tmp, mut lib) = setup();
        let state = tmp.path().join("state");
        receive(
            &mut lib,
            "A/Book.txt",
            b"book",
            json!({"uuid": "u1", "title": "Book", "authors": ["A"], "size": 4,
                   "last_modified": "2026-10-17T00:00:00+00:00", "series": "Saga",
                   "series_index": 1, "tags": ["x"], "user_metadata": {"#kindle_read": {}}}),
        );
        lib.books[0]["_kws"] =
            json!({"cde_key": "k", "cde_type": "PDOC", "read_sync": {"read": true}});
        lib.save().unwrap();
        lib.set_driveinfo(
            json!({"device_store_uuid": "dev-1", "device_name": "K", "last_library_uuid": "lib-a"}),
        )
        .unwrap();
        let before = lib.book("A/Book.txt").unwrap().clone();

        assert!(!lib.reset_if_relinked("lib-a").unwrap(), "same library");
        assert_eq!(lib.book("A/Book.txt").unwrap(), &before);

        assert!(lib.reset_if_relinked("lib-b").unwrap());
        // Like an adopted book, dated by its file, so any library resends.
        let modified = fs::metadata(lib.books_dir().join("A/Book.txt"))
            .unwrap()
            .modified()
            .unwrap();
        let expected = json!({"lpath": "A/Book.txt", "size": 4,
            "last_modified": adopt::iso_utc(modified), "uuid": "u1", "title": "Book",
            "authors": ["A"], "series": "Saga", "series_index": 1, "thumbnail": null,
            "_kws": {"cde_key": "k", "cde_type": "PDOC"}});
        assert_ne!(expected["last_modified"], before["last_modified"]);
        assert_eq!(lib.book("A/Book.txt").unwrap(), &expected);
        assert_eq!(lib.calibre_metadata(0).unwrap()["series"], "Saga");
        assert!(!state.join(DRIVEINFO_FILE).exists());
        assert_eq!(lib.driveinfo(), &json!({}));

        // The reset was saved, and with no driveinfo there's nothing to redo.
        let mut lib = Library::open(&tmp.path().join("books"), &state).unwrap();
        assert_eq!(lib.book("A/Book.txt").unwrap(), &expected);
        assert!(!lib.reset_if_relinked("lib-b").unwrap());
    }

    #[test]
    fn no_reset_unless_driveinfo_names_another_library() {
        let (_tmp, mut lib) = setup();
        receive(
            &mut lib,
            "Book.txt",
            b"book",
            json!({"uuid": "u1", "tags": ["x"]}),
        );
        assert!(!lib.reset_if_relinked("lib-b").unwrap(), "fresh install");
        for info in [
            json!({"device_store_uuid": "dev-1"}),
            json!({"device_store_uuid": "dev-1", "last_library_uuid": null}),
            json!({"device_store_uuid": "dev-1", "last_library_uuid": ""}),
        ] {
            lib.set_driveinfo(info).unwrap();
            assert!(!lib.reset_if_relinked("lib-b").unwrap());
        }
        assert_eq!(lib.book("Book.txt").unwrap()["tags"], json!(["x"]));
        assert_eq!(lib.driveinfo()["device_store_uuid"], "dev-1");
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

    fn enable_covers(tmp: &tempfile::TempDir, lib: &mut Library) -> PathBuf {
        let thumbs = tmp.path().join("thumbnails");
        fs::create_dir_all(&thumbs).unwrap();
        lib.enable_covers(CoverConfig {
            thumbnails_dir: thumbs.clone(),
            catalog_db: None,
        })
        .unwrap();
        thumbs
    }

    fn mobi_with_key(key: &str) -> Vec<u8> {
        crate::kindle::tests::fake_mobi(&[(113, key.as_bytes()), (501, b"EBOK")])
    }

    // "/9j/4AAQ" is the start of a JPEG: ff d8 ff e0 00 10.
    const JPEG_B64: &str = "/9j/4AAQ";
    const JPEG: [u8; 6] = [0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10];

    #[test]
    fn adopts_calibre_books_it_did_not_receive() {
        const UUID: &str = "cb5d2673-dbcf-4a0c-b9a6-90de29a24177";
        let (tmp, mut lib) = setup();
        let thumbs = enable_covers(&tmp, &mut lib);
        let exts: Vec<String> = ["azw3", "azw", "pdf"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // A USB-sent AZW3 with the cover the USB driver wrote, an Amazon
        // book, an EPUB, and a PDF listed in the USB driver's metadata.
        let book = crate::kindle::tests::fake_mobi(&[
            (112, format!("calibre:{UUID}").as_bytes()),
            (113, UUID.as_bytes()),
            (501, b"EBOK"),
            (503, b"Example Book"),
            (100, b"Jane Author"),
        ]);
        fs::write(
            lib.books_dir().join("Example Book - Jane Author.azw3"),
            &book,
        )
        .unwrap();
        let cover = format!("thumbnail_{UUID}_EBOK_portrait.jpg");
        fs::write(thumbs.join(&cover), JPEG).unwrap();
        fs::write(
            lib.books_dir().join("Store.azw"),
            mobi_with_key("B0053VMNYW"),
        )
        .unwrap();
        fs::write(lib.books_dir().join("Other.epub"), b"epub").unwrap();
        fs::write(lib.books_dir().join("Report.pdf"), b"%PDF-").unwrap();
        fs::write(
            tmp.path().join("metadata.calibre"),
            json!([{"lpath": "books/report.pdf", "uuid": UUID, "title": "Report", "size": 5}])
                .to_string(),
        )
        .unwrap();

        assert_eq!(lib.adopt_existing(&exts).unwrap(), 2);
        let b = lib.book("Example Book - Jane Author.azw3").unwrap();
        assert_eq!(b["uuid"], UUID);
        assert_eq!(b["title"], "Example Book");
        assert_eq!(b["_kws"]["cde_key"], UUID);
        assert_eq!(lib.book("Report.pdf").unwrap()["title"], "Report");
        assert!(lib.book("Store.azw").is_none());
        assert_eq!(
            fs::read(tmp.path().join("state/covers").join(&cover)).unwrap(),
            JPEG
        );
        // Nothing on disk changed, and a second scan finds nothing new.
        assert_eq!(
            fs::read(lib.books_dir().join("Example Book - Jane Author.azw3")).unwrap(),
            book
        );
        assert_eq!(lib.adopt_existing(&exts).unwrap(), 0);

        // Adopted books are saved, and deleting one works like any other.
        let mut lib2 = Library::open(&tmp.path().join("books"), &tmp.path().join("state")).unwrap();
        assert_eq!(lib2.books().len(), 2);
        assert_eq!(
            lib2.delete_book("Report.pdf").unwrap().as_deref(),
            Some(UUID)
        );
        assert!(!lib2.books_dir().join("Report.pdf").exists());
    }

    #[test]
    fn cover_and_backup_are_written_and_key_kept_private() {
        let (tmp, mut lib) = setup();
        let thumbs = enable_covers(&tmp, &mut lib);
        let meta = json!({"uuid": "u1", "thumbnail": [1, 1, JPEG_B64]});
        receive(&mut lib, "Book.azw3", &mobi_with_key("k1"), meta);

        let name = "thumbnail_k1_EBOK_portrait.jpg";
        assert_eq!(fs::read(thumbs.join(name)).unwrap(), JPEG);
        assert_eq!(
            fs::read(tmp.path().join("state/covers").join(name)).unwrap(),
            JPEG
        );
        assert_eq!(lib.book("Book.azw3").unwrap()["_kws"]["cde_key"], "k1");
        assert!(lib.calibre_metadata(0).unwrap().get("_kws").is_none());
        // Calibre's metadata updates keep our private key.
        assert!(lib.update_metadata(json!({"lpath": "Book.azw3", "title": "T"})));
        assert_eq!(lib.book("Book.azw3").unwrap()["_kws"]["cde_type"], "EBOK");
        // No temporary files left in the thumbnails directory.
        assert_eq!(fs::read_dir(&thumbs).unwrap().count(), 1);
    }

    #[test]
    fn lost_or_replaced_covers_are_restored() {
        let (tmp, mut lib) = setup();
        let thumbs = enable_covers(&tmp, &mut lib);
        let meta = json!({"thumbnail": [1, 1, JPEG_B64]});
        receive(&mut lib, "Book.azw3", &mobi_with_key("k1"), meta);
        let thumb = thumbs.join("thumbnail_k1_EBOK_portrait.jpg");

        assert_eq!(lib.restore_covers(), 0, "nothing to do while intact");
        fs::write(&thumb, b"placeholder from the firmware").unwrap();
        assert_eq!(lib.restore_covers(), 1);
        assert_eq!(fs::read(&thumb).unwrap(), JPEG);
        fs::remove_file(&thumb).unwrap();
        // Restoring also happens when covers are enabled at startup.
        let mut lib2 = Library::open(&tmp.path().join("books"), &tmp.path().join("state")).unwrap();
        enable_covers(&tmp, &mut lib2);
        assert_eq!(fs::read(&thumb).unwrap(), JPEG);
    }

    #[test]
    fn delete_removes_cover_and_backup() {
        let (tmp, mut lib) = setup();
        let thumbs = enable_covers(&tmp, &mut lib);
        receive(
            &mut lib,
            "Book.azw3",
            &mobi_with_key("k1"),
            json!({"thumbnail": [1, 1, JPEG_B64]}),
        );
        lib.delete_book("Book.azw3").unwrap();
        assert_eq!(fs::read_dir(&thumbs).unwrap().count(), 0);
        assert_eq!(
            fs::read_dir(tmp.path().join("state/covers"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn formats_without_covers_get_none() {
        let (tmp, mut lib) = setup();
        let thumbs = enable_covers(&tmp, &mut lib);
        let meta = json!({"thumbnail": [1, 1, JPEG_B64]});
        receive(&mut lib, "a.pdf", b"%PDF-1.4", meta.clone());
        // KFX needs the catalog, which is disabled here.
        receive(&mut lib, "b.kfx", b"CONT", meta.clone());
        // A MOBI without EXTH 113/501 also needs the catalog.
        receive(
            &mut lib,
            "c.mobi",
            &crate::kindle::tests::fake_mobi(&[]),
            meta,
        );
        assert_eq!(fs::read_dir(&thumbs).unwrap().count(), 0);
        assert!(lib.books().iter().all(|b| b.get("_kws").is_none()));
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
