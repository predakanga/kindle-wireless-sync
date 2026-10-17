// SPDX-License-Identifier: GPL-3.0-or-later
//! The Calibre library this Kindle is linked to (ADR 0018, which replaced the
//! allowlist of ADR 0003), and the user's settings. Stored as `config.json`
//! in the state directory.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::readsync::Columns;

pub const CONFIG_FILE: &str = "config.json";
const LOCK_FILE: &str = "config.lock";

/// An exclusive `flock` on the state directory's lock file, released when
/// dropped. `flock` locks belong to the open file, so this serialises
/// threads within the daemon as well as separate CLI processes.
struct ConfigLock {
    /// Kept open only to hold the lock.
    _file: File,
}

impl ConfigLock {
    fn acquire(state_dir: &Path) -> io::Result<ConfigLock> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(state_dir.join(LOCK_FILE))?;
        // SAFETY: the descriptor is valid for the lifetime of `file`.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(ConfigLock { _file: file })
    }
}

/// Loads the config, applies `f`, and saves it, holding the config lock.
/// Every change to `config.json` goes through here (ADR 0011); `f` should
/// use `Config`'s typed operations.
pub fn update<T>(state_dir: &Path, f: impl FnOnce(&mut Config) -> T) -> io::Result<T> {
    let _lock = ConfigLock::acquire(state_dir)?;
    let mut config = Config::load(state_dir)?;
    let result = f(&mut config);
    config.save()?;
    Ok(result)
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LinkedLibrary {
    pub uuid: String,
    pub name: String,
    /// Last address we reached it at. Tried directly by the background loop,
    /// so libraries outside the broadcast domain work.
    pub host: Option<String>,
    pub port: Option<u16>,
    /// Hostname from its discovery reply, used to match broadcast replies.
    pub hostname: Option<String>,
    pub password: Option<String>,
    /// Whether the Kindle may change this library: read status and
    /// last-read date (ADR 0017). Off unless set.
    pub writable: bool,
    /// Lookup names of the sync columns, used while `writable` is on.
    pub read_column: Option<String>,
    pub last_read_column: Option<String>,
}

impl LinkedLibrary {
    /// The sync columns to use with this library, if it's writable.
    pub fn sync_columns(&self) -> Option<Columns> {
        self.writable.then(|| Columns {
            read: self.read_column.clone(),
            last_read: self.last_read_column.clone(),
        })
    }

    /// Its name, or its UUID if Calibre gave no name.
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.uuid
        } else {
            &self.name
        }
    }
}

/// What [`Config::link`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum Link {
    /// Nothing was linked before.
    New,
    /// The same library: its address and name were updated.
    Updated,
    /// A different library was linked, and has been replaced.
    Replaced(LinkedLibrary),
    /// A different library is linked, and replacing it wasn't confirmed.
    /// Nothing changed.
    Refused(LinkedLibrary),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Config {
    pub library: Option<LinkedLibrary>,
    /// Calibre series as native series (ADR 0016). On unless set to false.
    pub series: bool,
    path: PathBuf,
}

fn opt_s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_owned)
}

impl Config {
    /// Loads the config. A missing file is an empty config; a corrupt one is
    /// an error, so a damaged file never silently unlinks the library.
    pub fn load(state_dir: &Path) -> io::Result<Config> {
        let path = state_dir.join(CONFIG_FILE);
        let value: Value = match fs::read(&path) {
            Ok(data) => serde_json::from_slice(&data).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {e}", path.display()),
                )
            })?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => json!({}),
            Err(e) => return Err(e),
        };
        let library = value
            .get("library")
            .filter(|l| l.is_object())
            .map(|l| LinkedLibrary {
                uuid: opt_s(l, "uuid").unwrap_or_default(),
                name: opt_s(l, "name").unwrap_or_default(),
                host: opt_s(l, "host"),
                port: l
                    .get("port")
                    .and_then(Value::as_u64)
                    .and_then(|p| u16::try_from(p).ok()),
                hostname: opt_s(l, "hostname"),
                password: opt_s(l, "password"),
                writable: l.get("writable").and_then(Value::as_bool).unwrap_or(false),
                read_column: opt_s(l, "read_column"),
                last_read_column: opt_s(l, "last_read_column"),
            })
            .filter(|l| !l.uuid.is_empty());
        Ok(Config {
            library,
            series: value.get("series").and_then(Value::as_bool).unwrap_or(true),
            path,
        })
    }

    pub fn save(&self) -> io::Result<()> {
        let value = json!({
            "library": self.library.as_ref().map(|a| json!({
                "uuid": a.uuid, "name": a.name, "host": a.host, "port": a.port,
                "hostname": a.hostname, "password": a.password,
                "writable": a.writable, "read_column": a.read_column,
                "last_read_column": a.last_read_column,
            })),
            "series": self.series,
        });
        let tmp = self.path.with_extension("json.tmp");
        fs::write(
            &tmp,
            serde_json::to_vec_pretty(&value).map_err(io::Error::from)?,
        )?;
        fs::rename(&tmp, &self.path)
    }

    /// The linked library, if it is the one with this UUID.
    pub fn linked(&self, uuid: &str) -> Option<&LinkedLibrary> {
        self.library.as_ref().filter(|a| a.uuid == uuid)
    }

    /// Links a library, or updates the linked one's address and name. For the
    /// same library this keeps the stored password unless a new one is given,
    /// and always keeps the sync settings: only [`Config::set_writable`]
    /// changes those. A different library replaces the linked one only with
    /// `replace`, and starts from `lib` alone (read-only unless it says
    /// otherwise, ADR 0017).
    pub fn link(&mut self, lib: LinkedLibrary, replace: bool) -> Link {
        match &mut self.library {
            Some(a) if a.uuid == lib.uuid => {
                let password = lib.password.clone().or_else(|| a.password.take());
                let hostname = lib.hostname.clone().or_else(|| a.hostname.take());
                *a = LinkedLibrary {
                    password,
                    hostname,
                    writable: a.writable,
                    read_column: a.read_column.take(),
                    last_read_column: a.last_read_column.take(),
                    ..lib
                };
                Link::Updated
            }
            Some(a) if !replace => Link::Refused(a.clone()),
            slot => match slot.replace(lib) {
                Some(old) => Link::Replaced(old),
                None => Link::New,
            },
        }
    }

    /// Unlinks the library. Returns false if none was linked.
    pub fn unlink(&mut self) -> bool {
        self.library.take().is_some()
    }

    /// Lets the Kindle change the linked library, through the given sync
    /// columns, or stops it (`None`, keeping the names). Returns false if
    /// that library isn't the linked one.
    pub fn set_writable(&mut self, uuid: &str, columns: Option<Columns>) -> bool {
        match self.library.as_mut().filter(|a| a.uuid == uuid) {
            Some(a) => {
                a.writable = columns.is_some();
                if let Some(c) = columns {
                    a.read_column = c.read;
                    a.last_read_column = c.last_read;
                }
                true
            }
            None => false,
        }
    }

    /// Sets or clears the linked library's password. Returns false if that
    /// library isn't the linked one.
    pub fn set_password(&mut self, uuid: &str, password: Option<String>) -> bool {
        match self.library.as_mut().filter(|a| a.uuid == uuid) {
            Some(a) => {
                a.password = password;
                true
            }
            None => false,
        }
    }

    /// Whether a discovered server is (as far as we know) the linked
    /// library's: same broadcast hostname or same address.
    pub fn matches_linked(&self, hostname: &str, host: &str) -> bool {
        self.library.as_ref().is_some_and(|a| {
            a.hostname.as_deref() == Some(hostname) || a.host.as_deref() == Some(host)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(uuid: &str) -> LinkedLibrary {
        LinkedLibrary {
            uuid: uuid.into(),
            name: format!("Library {uuid}"),
            host: Some("10.0.0.5".into()),
            port: Some(9090),
            ..Default::default()
        }
    }

    #[test]
    fn link_round_trips_and_updates_keep_password_and_sync_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let mut c = Config::load(tmp.path()).unwrap();
        assert_eq!(c.library, None);
        assert_eq!(
            c.link(
                LinkedLibrary {
                    password: Some("pw".into()),
                    hostname: Some("pc".into()),
                    ..lib("lib-1")
                },
                false
            ),
            Link::New
        );
        assert!(c.set_writable("lib-1", Some(Columns::defaults())));
        c.save().unwrap();

        let mut c = Config::load(tmp.path()).unwrap();
        // The daemon updating the address after a session.
        let update = LinkedLibrary {
            host: Some("10.0.0.6".into()),
            port: Some(9091),
            ..lib("lib-1")
        };
        assert_eq!(c.link(update, false), Link::Updated);
        let a = c.linked("lib-1").unwrap();
        assert_eq!(a.port, Some(9091));
        assert_eq!(a.password.as_deref(), Some("pw"));
        assert_eq!(a.hostname.as_deref(), Some("pc"));
        assert_eq!(a.sync_columns(), Some(Columns::defaults()));
        assert!(c.linked("lib-2").is_none());

        // Turning writable off keeps the names for next time.
        assert!(!c.set_writable("lib-2", None), "not the linked library");
        assert!(c.set_writable("lib-1", None));
        let a = c.linked("lib-1").unwrap();
        assert_eq!(a.sync_columns(), None);
        assert_eq!(a.read_column.as_deref(), Some("#kindle_read"));
    }

    #[test]
    fn a_different_library_replaces_only_when_confirmed() {
        let tmp = tempfile::tempdir().unwrap();
        let mut c = Config::load(tmp.path()).unwrap();
        c.link(
            LinkedLibrary {
                password: Some("pw".into()),
                ..lib("lib-1")
            },
            false,
        );
        c.set_writable("lib-1", Some(Columns::defaults()));
        let old = c.library.clone().unwrap();

        assert_eq!(c.link(lib("lib-2"), false), Link::Refused(old.clone()));
        assert_eq!(c.library.as_ref(), Some(&old));

        assert_eq!(c.link(lib("lib-2"), true), Link::Replaced(old));
        // A fresh start: no password, read-only.
        assert_eq!(c.library, Some(lib("lib-2")));
        assert_eq!(c.linked("lib-2").unwrap().sync_columns(), None);
    }

    #[test]
    fn unlink_password_and_update() {
        let tmp = tempfile::tempdir().unwrap();
        update(tmp.path(), |c| {
            c.link(
                LinkedLibrary {
                    hostname: Some("pc".into()),
                    host: Some("10.0.0.2".into()),
                    ..lib("lib-1")
                },
                false,
            )
        })
        .unwrap();
        assert!(update(tmp.path(), |c| c.set_password("lib-1", Some("pw".into()))).unwrap());
        assert!(!update(tmp.path(), |c| c.set_password("lib-2", None)).unwrap());
        let c = Config::load(tmp.path()).unwrap();
        assert_eq!(c.linked("lib-1").unwrap().password.as_deref(), Some("pw"));
        assert!(c.matches_linked("pc", "10.9.9.9"));
        assert!(c.matches_linked("other", "10.0.0.2"));
        assert!(!c.matches_linked("other", "10.9.9.9"));
        assert!(update(tmp.path(), |c| c.unlink()).unwrap());
        assert!(!update(tmp.path(), |c| c.unlink()).unwrap());
        let c = Config::load(tmp.path()).unwrap();
        assert_eq!(c.library, None);
        assert!(!c.matches_linked("pc", "10.0.0.2"));
    }

    #[test]
    fn concurrent_updates_are_not_lost() {
        let tmp = tempfile::tempdir().unwrap();
        update(tmp.path(), |c| c.link(lib("lib-1"), false)).unwrap();
        let dir = tmp.path().to_path_buf();
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    for i in 0..10 {
                        update(&dir, |c| {
                            let a = c.library.as_mut().unwrap();
                            a.name.push_str(&format!(" {t}.{i}"));
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let name = Config::load(&dir).unwrap().library.unwrap().name;
        assert_eq!(name.split_whitespace().count(), 2 + 80);
    }

    #[test]
    fn corrupt_config_is_an_error_not_an_unlinked_kindle() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(CONFIG_FILE), b"{not json").unwrap();
        assert!(Config::load(tmp.path()).is_err());
    }

    #[test]
    fn the_old_allowlist_is_not_migrated() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join(CONFIG_FILE),
            br#"{"allowed": [{"uuid": "lib-1"}], "seen": [], "series": false}"#,
        )
        .unwrap();
        let c = Config::load(tmp.path()).unwrap();
        assert_eq!(c.library, None);
        assert!(!c.series);
    }
}
