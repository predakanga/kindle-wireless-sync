//! The allowlist of Calibre libraries, and libraries found by "Find
//! libraries" but not yet allowed (ADR 0003, ADR 0008). Stored as
//! `config.json` in the state directory.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

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
pub struct AllowedLibrary {
    pub uuid: String,
    pub name: String,
    /// Last address we reached it at. Tried directly by the background loop,
    /// so libraries outside the broadcast domain work.
    pub host: Option<String>,
    pub port: Option<u16>,
    /// Hostname from its discovery reply, used to match broadcast replies.
    pub hostname: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SeenLibrary {
    pub uuid: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub hostname: String,
    /// Unix seconds.
    pub last_seen: u64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Config {
    pub allowed: Vec<AllowedLibrary>,
    pub seen: Vec<SeenLibrary>,
    path: PathBuf,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn opt_s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_owned)
}

fn port(v: &Value) -> Option<u16> {
    v.get("port")
        .and_then(Value::as_u64)
        .and_then(|p| u16::try_from(p).ok())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Config {
    /// Loads the config. A missing file is an empty config; a corrupt one is
    /// an error, so a damaged file never silently empties the allowlist.
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
        let list = |k: &str| {
            value
                .get(k)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        Ok(Config {
            allowed: list("allowed")
                .iter()
                .map(|a| AllowedLibrary {
                    uuid: s(a, "uuid"),
                    name: s(a, "name"),
                    host: opt_s(a, "host"),
                    port: port(a),
                    hostname: opt_s(a, "hostname"),
                    password: opt_s(a, "password"),
                })
                .filter(|a| !a.uuid.is_empty())
                .collect(),
            seen: list("seen")
                .iter()
                .map(|v| SeenLibrary {
                    uuid: s(v, "uuid"),
                    name: s(v, "name"),
                    host: s(v, "host"),
                    port: port(v).unwrap_or(0),
                    hostname: s(v, "hostname"),
                    last_seen: v.get("last_seen").and_then(Value::as_u64).unwrap_or(0),
                })
                .filter(|v| !v.uuid.is_empty())
                .collect(),
            path,
        })
    }

    pub fn save(&self) -> io::Result<()> {
        let value = json!({
            "allowed": self.allowed.iter().map(|a| json!({
                "uuid": a.uuid, "name": a.name, "host": a.host, "port": a.port,
                "hostname": a.hostname, "password": a.password,
            })).collect::<Vec<_>>(),
            "seen": self.seen.iter().map(|v| json!({
                "uuid": v.uuid, "name": v.name, "host": v.host, "port": v.port,
                "hostname": v.hostname, "last_seen": v.last_seen,
            })).collect::<Vec<_>>(),
        });
        let tmp = self.path.with_extension("json.tmp");
        fs::write(
            &tmp,
            serde_json::to_vec_pretty(&value).map_err(io::Error::from)?,
        )?;
        fs::rename(&tmp, &self.path)
    }

    pub fn allowed(&self, uuid: &str) -> Option<&AllowedLibrary> {
        self.allowed.iter().find(|a| a.uuid == uuid)
    }

    /// Allows a library found by "Find libraries", taking its name and
    /// address from `seen`. Returns false if it was already allowed or was
    /// never seen.
    pub fn allow_seen(&mut self, uuid: &str) -> bool {
        if self.allowed(uuid).is_some() {
            return false;
        }
        let Some(i) = self.seen.iter().position(|v| v.uuid == uuid) else {
            return false;
        };
        let v = self.seen.remove(i);
        self.allowed.push(AllowedLibrary {
            uuid: v.uuid,
            name: v.name,
            host: Some(v.host),
            port: Some(v.port),
            hostname: (!v.hostname.is_empty()).then_some(v.hostname),
            password: None,
        });
        true
    }

    /// Adds or updates an allowed library ("Add server", or after
    /// connecting). Keeps an existing password unless a new one is given.
    pub fn upsert_allowed(&mut self, lib: AllowedLibrary) {
        self.seen.retain(|v| v.uuid != lib.uuid);
        match self.allowed.iter_mut().find(|a| a.uuid == lib.uuid) {
            Some(a) => {
                let password = lib.password.clone().or_else(|| a.password.take());
                let hostname = lib.hostname.clone().or_else(|| a.hostname.take());
                *a = AllowedLibrary {
                    password,
                    hostname,
                    ..lib
                };
            }
            None => self.allowed.push(lib),
        }
    }

    /// Removes an allowed library. Returns false if it wasn't allowed.
    pub fn forget(&mut self, uuid: &str) -> bool {
        let before = self.allowed.len();
        self.allowed.retain(|a| a.uuid != uuid);
        self.allowed.len() != before
    }

    /// Sets or clears an allowed library's password. Returns false if the
    /// library isn't allowed.
    pub fn set_password(&mut self, uuid: &str, password: Option<String>) -> bool {
        match self.allowed.iter_mut().find(|a| a.uuid == uuid) {
            Some(a) => {
                a.password = password;
                true
            }
            None => false,
        }
    }

    /// Whether a discovered server is (as far as we know) an allowed
    /// library's: same broadcast hostname or same address.
    pub fn matches_allowed(&self, hostname: &str, host: &str) -> bool {
        self.allowed
            .iter()
            .any(|a| a.hostname.as_deref() == Some(hostname) || a.host.as_deref() == Some(host))
    }

    /// Records a library that answered but is not allowed.
    pub fn record_seen(&mut self, uuid: &str, name: &str, host: &str, port: u16, hostname: &str) {
        let entry = SeenLibrary {
            uuid: uuid.to_owned(),
            name: name.to_owned(),
            host: host.to_owned(),
            port,
            hostname: hostname.to_owned(),
            last_seen: now_secs(),
        };
        match self.seen.iter_mut().find(|v| v.uuid == uuid) {
            Some(v) => *v = entry,
            None => self.seen.push(entry),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_moves_seen_to_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let mut c = Config::load(tmp.path()).unwrap();
        assert!(c.allowed.is_empty());
        c.record_seen("lib-1", "Calibre Library", "192.168.1.2", 9090, "pc");
        assert!(c.allow_seen("lib-1"));
        assert!(!c.allow_seen("lib-1"), "already allowed");
        assert!(!c.allow_seen("never-seen"));
        c.upsert_allowed(AllowedLibrary {
            uuid: "lib-2".into(),
            name: "Other".into(),
            host: Some("10.0.0.5".into()),
            port: Some(9090),
            hostname: None,
            password: Some("pw".into()),
        });
        c.save().unwrap();

        let mut c = Config::load(tmp.path()).unwrap();
        assert!(c.seen.is_empty());
        let a = c.allowed("lib-1").unwrap();
        assert_eq!(a.name, "Calibre Library");
        assert_eq!(a.host.as_deref(), Some("192.168.1.2"));
        assert_eq!(a.hostname.as_deref(), Some("pc"));

        // Updating an address keeps the stored password.
        c.upsert_allowed(AllowedLibrary {
            uuid: "lib-2".into(),
            name: "Other".into(),
            host: Some("10.0.0.6".into()),
            port: Some(9091),
            ..Default::default()
        });
        let b = c.allowed("lib-2").unwrap();
        assert_eq!(b.password.as_deref(), Some("pw"));
        assert_eq!(b.port, Some(9091));
    }

    #[test]
    fn forget_password_and_update() {
        let tmp = tempfile::tempdir().unwrap();
        update(tmp.path(), |c| {
            c.upsert_allowed(AllowedLibrary {
                uuid: "lib-1".into(),
                hostname: Some("pc".into()),
                host: Some("10.0.0.2".into()),
                ..Default::default()
            })
        })
        .unwrap();
        assert!(update(tmp.path(), |c| c.set_password("lib-1", Some("pw".into()))).unwrap());
        let c = Config::load(tmp.path()).unwrap();
        assert_eq!(c.allowed("lib-1").unwrap().password.as_deref(), Some("pw"));
        assert!(c.matches_allowed("pc", "10.9.9.9"));
        assert!(c.matches_allowed("other", "10.0.0.2"));
        assert!(!c.matches_allowed("other", "10.9.9.9"));
        assert!(update(tmp.path(), |c| c.forget("lib-1")).unwrap());
        assert!(!update(tmp.path(), |c| c.forget("lib-1")).unwrap());
        assert!(Config::load(tmp.path()).unwrap().allowed.is_empty());
    }

    #[test]
    fn concurrent_updates_are_not_lost() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    for i in 0..10 {
                        let uuid = format!("lib-{t}-{i}");
                        update(&dir, |c| c.record_seen(&uuid, "n", "h", 1, "x")).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(Config::load(&dir).unwrap().seen.len(), 80);
    }

    #[test]
    fn corrupt_config_is_an_error_not_an_empty_allowlist() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(CONFIG_FILE), b"{not json").unwrap();
        assert!(Config::load(tmp.path()).is_err());
    }
}
