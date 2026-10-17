// SPDX-License-Identifier: GPL-3.0-or-later
//! Checking a Calibre library before linking it (ADR 0020): is it there,
//! does it take the password, and which columns does it have. A check
//! connects under its own device ID and syncs nothing.

use std::fs;
use std::io::{self, BufReader, BufWriter};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::daemon::{self, FoundLibrary};
use crate::library;
use crate::protocol::Connection;
use crate::session::{self, CheckEnd, DeviceConfig};

/// The check's own device ID, made by Calibre at the first check and reused
/// by every check after it, so Calibre keeps one cache file for checks.
const CHECK_ID_FILE: &str = "check_device_uuid";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Calibre sends a keep-alive about every 3 s while connected.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// What to tell the user when Calibre closes a connection before saying
/// anything: most likely its lockout after 5 failed connections from this
/// address, which only restarting its wireless connection clears.
pub const LOCKED_OUT: &str = "Calibre may have stopped accepting this Kindle after too many \
    failed attempts. In Calibre, stop and start the wireless device connection \
    (Connect/share), then try again.";

/// The library to check.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// One that Find libraries found: its name and whether it wants a
    /// password are already known.
    Found(FoundLibrary),
    /// One entered by address.
    Address { host: String, port: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Checking,
    Ok,
    PasswordNeeded,
    PasswordRejected,
    Unreachable,
    Busy,
    LockedOut,
    Failed,
}

impl CheckState {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckState::Checking => "checking",
            CheckState::Ok => "ok",
            CheckState::PasswordNeeded => "password_needed",
            CheckState::PasswordRejected => "password_rejected",
            CheckState::Unreachable => "unreachable",
            CheckState::Busy => "busy",
            CheckState::LockedOut => "locked_out",
            CheckState::Failed => "error",
        }
    }
}

/// A library that passed a check, ready to link.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    pub uuid: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub hostname: Option<String>,
    /// The password that worked, if Calibre wanted one.
    pub password: Option<String>,
    /// Lookup name to `{"name", "datatype"}`.
    pub columns: Map<String, Value>,
}

/// A check's progress or result.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckStatus {
    pub state: CheckState,
    /// What's being checked: the library's name once known, else the address.
    pub name: String,
    pub host: String,
    pub port: u16,
    pub uuid: Option<String>,
    pub error: Option<String>,
    pub library: Option<Checked>,
}

impl CheckStatus {
    /// The starting status for a target.
    pub fn checking(target: &Target) -> CheckStatus {
        match target {
            Target::Found(f) => CheckStatus {
                state: CheckState::Checking,
                name: f.name.clone(),
                host: f.host.clone(),
                port: f.port,
                uuid: Some(f.uuid.clone()),
                error: None,
                library: None,
            },
            Target::Address { host, port } => CheckStatus {
                state: CheckState::Checking,
                name: format!("{host}:{port}"),
                host: host.clone(),
                port: *port,
                uuid: None,
                error: None,
                library: None,
            },
        }
    }

    fn ended(mut self, state: CheckState, error: Option<String>) -> CheckStatus {
        self.state = state;
        self.error = error;
        self
    }

    /// For `/status`. The password stays out.
    pub fn to_json(&self) -> Value {
        json!({
            "state": self.state.as_str(),
            "target": {"name": self.name, "host": self.host, "port": self.port, "uuid": self.uuid},
            "error": self.error,
            "library": self.library.as_ref().map(|l| json!({
                "uuid": l.uuid, "name": l.name, "host": l.host, "port": l.port,
                "hostname": l.hostname, "columns": l.columns,
            })),
        })
    }
}

/// What a check needs from the service.
pub struct Env {
    pub state_dir: PathBuf,
    pub books_dir: PathBuf,
    pub device: DeviceConfig,
    /// How long Calibre must be quiet before the check leaves.
    pub quiet: Duration,
}

/// Runs a check to the end. `hold` is called with the address before the
/// first connection, and with `None` once the check is done: the service
/// uses it to step aside if its own session is with the same Calibre, which
/// would otherwise answer "busy".
pub fn run(
    env: &Env,
    target: &Target,
    password: Option<String>,
    hold: &dyn Fn(Option<SocketAddr>),
) -> CheckStatus {
    let mut status = CheckStatus::checking(target);
    let addr = match (status.host.as_str(), status.port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
    {
        Some(a) => a,
        None => {
            let error = format!("Couldn't find {} on the network.", status.host);
            return status.ended(CheckState::Unreachable, Some(error));
        }
    };

    // A library Find has seen: whether it wants a password is known, which
    // saves a connection Calibre would count towards its lockout.
    if let Target::Found(f) = target {
        if f.password_required && password.is_none() {
            return status.ended(CheckState::PasswordNeeded, None);
        }
    }

    hold(Some(addr));
    let _release = Release(hold);
    let password_required = match target {
        Target::Found(f) => f.password_required,
        Target::Address { .. } => match daemon::probe(addr) {
            Ok(info) => {
                status.name = info.library_name.clone();
                status.uuid = Some(info.library_uuid.clone());
                info.password_required
            }
            Err(e) => return probe_failed(status, &e),
        },
    };
    if password_required && password.is_none() {
        return status.ended(CheckState::PasswordNeeded, None);
    }

    match full_check(env, addr, password.as_deref()) {
        Ok(CheckEnd::Done {
            info,
            columns,
            device_uuid,
        }) => {
            if let Some(id) = device_uuid {
                if let Err(e) = fs::write(env.state_dir.join(CHECK_ID_FILE), &id) {
                    log::warn!("cannot keep the check's device ID: {e}");
                }
            }
            if status
                .uuid
                .as_deref()
                .is_some_and(|u| u != info.library_uuid)
            {
                // Calibre switched libraries since Find saw it.
                log::info!(
                    "{addr} now serves {:?} instead of {:?}",
                    info.library_name,
                    status.name
                );
            }
            status.name = info.library_name.clone();
            status.uuid = Some(info.library_uuid.clone());
            status.library = Some(Checked {
                uuid: info.library_uuid,
                name: info.library_name,
                host: status.host.clone(),
                port: status.port,
                hostname: match target {
                    Target::Found(f) if !f.hostname.is_empty() => Some(f.hostname.clone()),
                    _ => None,
                },
                password,
                columns,
            });
            status.ended(CheckState::Ok, None)
        }
        Ok(CheckEnd::PasswordNeeded(_)) => status.ended(CheckState::PasswordNeeded, None),
        Ok(CheckEnd::PasswordRejected(_)) => status.ended(CheckState::PasswordRejected, None),
        Ok(CheckEnd::Busy) => status.ended(
            CheckState::Busy,
            Some("Calibre is busy with another device.".into()),
        ),
        Ok(CheckEnd::ClosedAtOnce) => status.ended(CheckState::LockedOut, Some(LOCKED_OUT.into())),
        Ok(CheckEnd::ProtocolMismatch) => status.ended(
            CheckState::Failed,
            Some("This version of Calibre's wireless connection isn't supported.".into()),
        ),
        Ok(CheckEnd::Closed) => status.ended(
            CheckState::Failed,
            Some("Calibre ended the connection before the check finished.".into()),
        ),
        Err(e) => {
            let state = if matches!(
                e.kind(),
                io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::TimedOut
                    | io::ErrorKind::WouldBlock
            ) {
                CheckState::Unreachable
            } else {
                CheckState::Failed
            };
            let text = unreachable_text(&status.host, status.port, &e);
            status.ended(state, Some(text))
        }
    }
}

/// Releases the hold when the check ends, whichever way.
struct Release<'a>(&'a dyn Fn(Option<SocketAddr>));

impl Drop for Release<'_> {
    fn drop(&mut self) {
        (self.0)(None);
    }
}

fn full_check(env: &Env, addr: SocketAddr, password: Option<&str>) -> io::Result<CheckEnd> {
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_nodelay(true)?;
    let mut conn = Connection::new(BufReader::new(stream.try_clone()?), BufWriter::new(stream));
    let id = check_id(&env.state_dir);
    let free = library::free_space_at(&env.books_dir).unwrap_or(0);
    session::check(
        &mut conn,
        &env.device,
        id.as_deref(),
        password,
        free,
        env.quiet,
    )
}

fn check_id(state_dir: &Path) -> Option<String> {
    let id = fs::read_to_string(state_dir.join(CHECK_ID_FILE)).ok()?;
    let id = id.trim();
    (!id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .then(|| id.to_owned())
}

fn probe_failed(status: CheckStatus, e: &io::Error) -> CheckStatus {
    if daemon::is_locked_out(e) {
        return status.ended(CheckState::LockedOut, Some(LOCKED_OUT.into()));
    }
    if daemon::is_busy(e) {
        return status.ended(
            CheckState::Busy,
            Some("Calibre is busy with another device.".into()),
        );
    }
    let text = unreachable_text(&status.host, status.port, e);
    status.ended(CheckState::Unreachable, Some(text))
}

fn unreachable_text(host: &str, port: u16, e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::ConnectionRefused | io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => {
            format!(
                "Nothing answered at {host}:{port}. Check that Calibre's wireless device connection is started."
            )
        }
        _ => format!("Couldn't connect to {host}:{port}: {e}"),
    }
}

/// The latest check, for the API. A new check or a cancel replaces it, and
/// a running check's result is dropped if it was replaced meanwhile.
#[derive(Default)]
pub struct Checker {
    inner: Mutex<(u64, Option<CheckStatus>)>,
}

impl Checker {
    /// Records a new check as running, and returns its number.
    pub fn begin(&self, status: CheckStatus) -> u64 {
        let mut inner = self.inner.lock().unwrap();
        inner.0 += 1;
        inner.1 = Some(status);
        inner.0
    }

    /// Records a check's result, unless another check or a cancel came since.
    pub fn finish(&self, number: u64, status: CheckStatus) {
        let mut inner = self.inner.lock().unwrap();
        if inner.0 == number {
            inner.1 = Some(status);
        }
    }

    pub fn cancel(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.0 += 1;
        inner.1 = None;
    }

    pub fn status(&self) -> Option<CheckStatus> {
        self.inner.lock().unwrap().1.clone()
    }

    /// The library that passed the latest check, if it did.
    pub fn checked(&self) -> Option<Checked> {
        self.status()
            .filter(|s| s.state == CheckState::Ok)
            .and_then(|s| s.library)
    }
}
