// SPDX-License-Identifier: GPL-3.0-or-later
//! The device side of one Calibre wireless session.
//!
//! Calibre drives the conversation: it sends a request, we answer. The order
//! and reply shapes mirror what Calibre's smart device driver expects; see
//! `docs/research/calibre-wireless-protocol.md` for the walkthrough.

use std::io::{self, BufRead, Write};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::library::Library;
use crate::lpath;
use crate::protocol::{Connection, Opcode};
use crate::readsync::{self, Columns};

pub const APP_NAME: &str = "Kindle Wireless Sync";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOL_VERSION: u64 = 1;
/// Chunk size Calibre uses when streaming book bytes to us.
const MAX_BOOK_PACKET: u64 = 64 * 1024;
/// How long Calibre must have sent nothing but keep-alives before we may
/// drop the connection without it noticing. For 5 s after an operation it
/// sends a keep-alive on every probe, and those can fall between two of its
/// jobs; dropping then fails the next job with an error dialog.
pub const QUIET_BEFORE_RECONNECT: Duration = Duration::from_secs(10);

/// Formats the built-in reader opens, in Calibre's order of preference for
/// conversion.
pub const DEFAULT_EXTENSIONS: &[&str] = &["azw3", "mobi", "azw", "kfx", "pdf", "txt"];

#[derive(Debug, Clone)]
pub struct DeviceConfig {
    pub device_name: String,
    pub device_kind: String,
    pub accepted_extensions: Vec<String>,
    pub cover_height: u32,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        DeviceConfig {
            device_name: "Kindle".into(),
            device_kind: "Kindle".into(),
            accepted_extensions: DEFAULT_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
            cover_height: 500,
        }
    }
}

/// What Calibre tells us about itself in the handshake.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CalibreInfo {
    pub library_uuid: String,
    pub library_name: String,
    pub calibre_version: String,
    /// Whether Calibre sent a password challenge: it has a password set.
    pub password_required: bool,
}

impl CalibreInfo {
    /// Reads the library identity from a `GET_INITIALIZATION_INFO` payload.
    pub fn from_init_payload(p: &Value) -> CalibreInfo {
        CalibreInfo {
            library_uuid: str_field(p, "currentLibraryUUID"),
            library_name: str_field(p, "currentLibraryName"),
            calibre_version: version_string(&p["calibre_version"]),
            password_required: !str_field(p, "passwordChallenge").is_empty(),
        }
    }
}

/// The answer to "may we talk to this library?", with the password it needs
/// and, if the Kindle may change it, the sync columns to use (ADR 0017).
#[derive(Debug, Clone, PartialEq)]
pub enum Admission {
    Reject,
    Accept {
        password: Option<String>,
        columns: Option<Columns>,
        /// The library's custom columns as last seen ([`Admit::columns_seen`]).
        /// The handshake names only sync columns listed here, because it
        /// comes before Calibre sends the list.
        known_columns: Map<String, Value>,
    },
}

/// What a session asks of whoever runs it: whether to talk to the library
/// Calibre announces, and somewhere to keep the library's columns for the
/// next handshake. A closure answering the first is enough when nothing
/// needs keeping.
pub trait Admit {
    fn admit(&mut self, info: &CalibreInfo) -> Admission;

    /// The library's custom columns, from `SET_LIBRARY_INFO`, when they
    /// differ from the known ones ([`readsync::custom_columns`]).
    fn columns_seen(&mut self, _columns: &Map<String, Value>) -> io::Result<()> {
        Ok(())
    }

    /// Asked at each keep-alive: whether to end the session as soon as
    /// Calibre is quiet, for a check of the same Calibre (ADR 0020).
    fn end_when_quiet(&mut self) -> bool {
        false
    }

    /// Asked at each keep-alive: whether to end the session now, quiet or
    /// not, because the user asked to (ADR 0022).
    fn leave_now(&mut self) -> bool {
        false
    }

    /// Where new books go, relative to the books folder (ADR 0023). Asked
    /// for each book, so a change applies at once.
    fn books_folder(&mut self) -> String {
        String::new()
    }
}

impl<F: FnMut(&CalibreInfo) -> Admission> Admit for F {
    fn admit(&mut self, info: &CalibreInfo) -> Admission {
        self(info)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEnd {
    /// Calibre disconnected us deliberately.
    Ejected,
    /// The stream ended.
    Closed,
    /// Calibre is already serving another device.
    Busy,
    PasswordRejected,
    /// The library is not on our allowlist (ADR 0003).
    LibraryRejected(CalibreInfo),
    ProtocolMismatch,
    /// The sync columns we can name changed (ADR 0017). Calibre only reads
    /// them in the handshake, so reconnect now. Returned only once Calibre
    /// has been idle, so it doesn't report an error.
    ColumnsChanged,
    /// Ended at a quiet moment because [`Admit::end_when_quiet`] asked.
    Yielded,
    /// Ended at a keep-alive because [`Admit::leave_now`] asked.
    Left,
    /// Calibre closed the connection before sending anything: most likely
    /// its lockout after too many failed connections (ADR 0020).
    ClosedAtOnce,
    /// Calibre switched to another library while we were connected: it
    /// keeps the device connected and only sends the new library's info
    /// (`set_current_library_information` in `gui2/device.py`). Ended at a
    /// quiet moment, like [`SessionEnd::ColumnsChanged`].
    LibraryChanged(CalibreInfo),
}

fn protocol_error(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Runs a session until Calibre disconnects. `admit` decides whether to stay
/// connected to the library Calibre announces, and with which password.
pub fn run<R: BufRead, W: Write>(
    conn: &mut Connection<R, W>,
    lib: &mut Library,
    cfg: &DeviceConfig,
    admit: &mut dyn Admit,
) -> io::Result<SessionEnd> {
    run_with_quiet(conn, lib, cfg, admit, QUIET_BEFORE_RECONNECT)
}

/// [`run`], with the idle time to wait before a reconnect (for tests).
pub fn run_with_quiet<R: BufRead, W: Write>(
    conn: &mut Connection<R, W>,
    lib: &mut Library,
    cfg: &DeviceConfig,
    admit: &mut dyn Admit,
    quiet: Duration,
) -> io::Result<SessionEnd> {
    let mut adopted = false;
    // The library's columns as last seen; replaced when Calibre lists others.
    let mut known = Map::new();
    // The sync columns this library should have, and those we named in the
    // handshake because we've seen them in the library (ADR 0017).
    let mut wanted: Option<Columns> = None;
    let mut named = Columns::default();
    // Read-status fields for the book list, by book index; kept for
    // Calibre's follow-up requests for full metadata.
    let mut sync_fields: Vec<Map<String, Value>> = Vec::new();
    // Set when the columns we can name changed: reconnect once Calibre is
    // idle. `last_busy` is when Calibre last sent anything but a keep-alive.
    let mut reconnect = false;
    let mut last_busy = Instant::now();
    let mut heard = false;
    // The library admitted in the handshake, and another one Calibre
    // switched to since (ADR 0018: only the linked library).
    let mut admitted = String::new();
    let mut switched: Option<CalibreInfo> = None;
    loop {
        let Some(msg) = conn.recv()? else {
            return Ok(if heard {
                SessionEnd::Closed
            } else {
                SessionEnd::ClosedAtOnce
            });
        };
        heard = true;
        let Some(op) = msg.opcode() else {
            log::warn!("ignoring unknown opcode {}", msg.code);
            continue;
        };
        let p = &msg.payload;
        let keep_alive = op == Opcode::Noop && p.as_object().is_none_or(Map::is_empty);
        if !keep_alive {
            last_busy = Instant::now();
        }
        match op {
            Opcode::GetInitializationInfo => {
                let info = CalibreInfo::from_init_payload(p);
                let version_ok = p["serverProtocolVersion"].as_u64() == Some(PROTOCOL_VERSION);
                let admission = if version_ok {
                    admit.admit(&info)
                } else {
                    Admission::Accept {
                        password: None,
                        columns: None,
                        known_columns: Map::new(),
                    }
                };
                let Admission::Accept {
                    password,
                    columns,
                    known_columns,
                } = admission
                else {
                    log::warn!(
                        "library {:?} ({}) is not allowed",
                        info.library_name,
                        info.library_uuid
                    );
                    return Ok(SessionEnd::LibraryRejected(info));
                };
                log::info!(
                    "connected to calibre {} library {:?}",
                    info.calibre_version,
                    info.library_name
                );
                admitted = info.library_uuid.clone();
                named = columns
                    .as_ref()
                    .map(|c| c.available_in(&known_columns))
                    .unwrap_or_default();
                known = known_columns;
                wanted = columns;
                conn.send(
                    Opcode::Ok,
                    init_info(cfg, p, version_ok, password.as_deref(), &named),
                )?;
                if !version_ok {
                    return Ok(SessionEnd::ProtocolMismatch);
                }
            }
            Opcode::GetDeviceInformation => {
                let mut device_info = lib.driveinfo().clone();
                if !device_info.is_object() {
                    device_info = json!({});
                }
                if device_info.get("device_name").is_none() {
                    device_info["device_name"] = json!(cfg.device_name);
                }
                conn.send(
                    Opcode::Ok,
                    json!({"device_info": device_info, "version": VERSION, "device_version": VERSION}),
                )?;
            }
            Opcode::SetCalibreDeviceInfo => {
                lib.set_driveinfo(p.clone())?;
                conn.send(Opcode::Ok, json!({}))?;
            }
            Opcode::SetCalibreDeviceName => {
                let mut info = lib.driveinfo().clone();
                if let (Some(obj), Some(name)) = (info.as_object_mut(), p.get("name")) {
                    obj.insert("device_name".into(), name.clone());
                    lib.set_driveinfo(info)?;
                }
                conn.send(Opcode::Ok, json!({}))?;
            }
            Opcode::TotalSpace => {
                conn.send(
                    Opcode::Ok,
                    json!({"total_space_on_device": lib.total_space()?}),
                )?;
            }
            Opcode::FreeSpace => {
                conn.send(
                    Opcode::Ok,
                    json!({"free_space_on_device": lib.free_space()?}),
                )?;
            }
            Opcode::SetLibraryInfo => {
                conn.send(Opcode::Ok, json!({}))?;
                let uuid = str_field(p, "libraryUuid");
                if !uuid.is_empty() && uuid != admitted {
                    // Not the linked library's columns: don't keep them.
                    if switched.is_none() {
                        log::warn!(
                            "calibre switched to library {:?}, which is not linked; \
                             disconnecting when calibre is idle",
                            str_field(p, "libraryName")
                        );
                    }
                    switched = Some(CalibreInfo {
                        library_uuid: uuid,
                        library_name: str_field(p, "libraryName"),
                        ..Default::default()
                    });
                    continue;
                }
                let custom = readsync::custom_columns(&p["fieldMetadata"]);
                let available = wanted.as_ref().map(|w| w.available_in(&custom));
                let recorded = if custom == known {
                    Ok(())
                } else {
                    admit.columns_seen(&custom)
                };
                match recorded {
                    Ok(()) => {
                        known = custom;
                        if available.is_some_and(|a| a != named) {
                            log::info!(
                                "the library's sync columns changed; reconnecting when calibre is idle"
                            );
                            reconnect = true;
                        }
                    }
                    // Without the record, a reconnect would name the same
                    // columns again; carry on with what we have.
                    Err(e) => log::warn!("cannot record the library's columns: {e}"),
                }
            }
            Opcode::GetBookCount => {
                // Once per session, before Calibre first sees the list:
                // books sent over USB since the last session (ADR 0013).
                if !adopted {
                    adopted = true;
                    match lib.adopt_existing(&cfg.accepted_extensions) {
                        Ok(0) => {}
                        Ok(n) => log::info!("adopted {n} books already on the device"),
                        Err(e) => log::warn!("adopting books failed: {e}"),
                    }
                }
                sync_fields = lib.read_sync_fields(&named).unwrap_or_else(|e| {
                    log::warn!("cannot save read-sync state: {e}");
                    Vec::new()
                });
                send_book_list(conn, lib, p, &sync_fields)?
            }
            Opcode::Noop => {
                if p.get("ejecting").and_then(Value::as_bool) == Some(true) {
                    conn.send(Opcode::Ok, json!({}))?;
                    return Ok(SessionEnd::Ejected);
                } else if let Some(index) = p.get("priKey").and_then(Value::as_u64) {
                    // Calibre's cache missed this book; send full metadata.
                    let index = index as usize;
                    let book = lib
                        .calibre_metadata(index)
                        .map(|b| with_fields(b, sync_fields.get(index)))
                        .unwrap_or_else(|| json!({}));
                    conn.send(Opcode::Ok, book)?;
                } else if p.get("count").is_some() {
                    // Announces how many priKey requests follow; no reply.
                } else {
                    // Calibre's keep-alive comes only between its jobs, from
                    // a check that takes a failure as the device leaving.
                    // So end here without replying (ADR 0022).
                    let end = if admit.leave_now() {
                        Some(SessionEnd::Left)
                    } else if last_busy.elapsed() < quiet {
                        None
                    } else if let Some(info) = switched.take() {
                        Some(SessionEnd::LibraryChanged(info))
                    } else if reconnect {
                        Some(SessionEnd::ColumnsChanged)
                    } else if admit.end_when_quiet() {
                        Some(SessionEnd::Yielded)
                    } else {
                        None
                    };
                    if let Some(end) = end {
                        return Ok(end);
                    }
                    conn.send(Opcode::Ok, json!({}))?;
                    // Calibre's keep-alive: a quiet moment between its
                    // operations, so run a due series pass (ADR 0016).
                    lib.reconcile_series_if_due();
                }
            }
            Opcode::SendBooklists => {
                let collections = p["collections"].as_object().map_or(0, Map::len);
                log::info!(
                    "calibre will send metadata for {} books, {collections} collections",
                    p["count"]
                );
            }
            Opcode::SendBookMetadata => {
                if !lib.update_metadata(p["data"].clone()) {
                    log::debug!(
                        "metadata for a book we do not hold: {:?}",
                        p["data"].get("lpath")
                    );
                }
                if p["index"].as_u64().map(|i| i + 1) == p["count"].as_u64() {
                    lib.save()?;
                }
            }
            Opcode::SendBook => {
                let folder = admit.books_folder();
                receive_book(conn, lib, cfg, p, &folder)?
            }
            Opcode::DeleteBook => {
                conn.send(Opcode::Ok, json!({}))?;
                let lpaths = p["lpaths"].as_array().cloned().unwrap_or_default();
                for l in &lpaths {
                    let l = l.as_str().unwrap_or_default();
                    let uuid = match lib.delete_book(l) {
                        Ok(Some(uuid)) => {
                            log::info!("deleted {l}");
                            uuid
                        }
                        Ok(None) => {
                            log::warn!("asked to delete {l:?}, which we do not hold");
                            String::new()
                        }
                        Err(e) => {
                            log::error!("failed to delete {l:?}: {e}");
                            String::new()
                        }
                    };
                    conn.send(Opcode::Ok, json!({"uuid": uuid}))?;
                }
                // All replied to: now take them out of the Library at once.
                lib.flush_catalog_deletes();
            }
            Opcode::GetBookFileSegment => {
                let l = str_field(p, "lpath");
                match lib.open_book(&l) {
                    Ok((mut file, len)) => {
                        conn.send(Opcode::Ok, json!({"fileLength": len}))?;
                        conn.send_raw(len, &mut file)?;
                    }
                    Err(e) => {
                        log::warn!("cannot send {l:?} to calibre: {e}");
                        conn.send(Opcode::Error, json!({"message": e.to_string()}))?;
                    }
                }
            }
            Opcode::DisplayMessage => {
                // Calibre waits for a reply, with no timeout: without one
                // its device thread hangs (seen with a missing sync column).
                conn.send(Opcode::Ok, json!({}))?;
                match p["messageKind"].as_u64() {
                    Some(1) => return Ok(SessionEnd::PasswordRejected),
                    _ => log::info!("calibre says: {}", p["message"]),
                }
            }
            Opcode::CalibreBusy => return Ok(SessionEnd::Busy),
            Opcode::Ok
            | Opcode::Error
            | Opcode::BookDone
            | Opcode::GetBookMetadata
            | Opcode::GetCollections
            | Opcode::UpdateCollections => log::warn!("ignoring unexpected {op}"),
        }
    }
}

/// How a check ended (ADR 0020).
#[derive(Debug, Clone, PartialEq)]
pub enum CheckEnd {
    /// The password, if any, was accepted and Calibre listed the library's
    /// columns. `device_uuid` is the device ID Calibre gave the check, when
    /// it had none.
    Done {
        info: CalibreInfo,
        columns: Map<String, Value>,
        device_uuid: Option<String>,
    },
    /// Calibre wants a password and none was given. Nothing was answered.
    PasswordNeeded(CalibreInfo),
    PasswordRejected(CalibreInfo),
    Busy,
    ProtocolMismatch,
    /// Calibre closed the connection before sending anything: most likely
    /// its lockout after too many failed connections from this address.
    ClosedAtOnce,
    /// Calibre closed the connection before listing the library's columns.
    Closed,
}

/// Checks a library without syncing (ADR 0020): connects as a device with
/// its own ID (`device_uuid`, or none so Calibre assigns one), names no
/// sync columns, lists no books and stores nothing Calibre sends. Ends at
/// the first keep-alive after `quiet` without other requests, which
/// Calibre takes without an error.
pub fn check<R: BufRead, W: Write>(
    conn: &mut Connection<R, W>,
    cfg: &DeviceConfig,
    device_uuid: Option<&str>,
    password: Option<&str>,
    free_space: u64,
    quiet: Duration,
) -> io::Result<CheckEnd> {
    let mut info: Option<CalibreInfo> = None;
    let mut columns: Option<Map<String, Value>> = None;
    let mut assigned: Option<String> = None;
    let mut listed = false;
    let mut last_busy = Instant::now();
    let mut heard = false;
    loop {
        let Some(msg) = conn.recv()? else {
            return Ok(match (heard, info, columns) {
                (false, _, _) => CheckEnd::ClosedAtOnce,
                (true, Some(info), Some(columns)) => CheckEnd::Done {
                    info,
                    columns,
                    device_uuid: assigned,
                },
                _ => CheckEnd::Closed,
            });
        };
        heard = true;
        let Some(op) = msg.opcode() else {
            log::warn!("check: ignoring unknown opcode {}", msg.code);
            continue;
        };
        let p = &msg.payload;
        let keep_alive = op == Opcode::Noop && p.as_object().is_none_or(Map::is_empty);
        if !keep_alive {
            last_busy = Instant::now();
        }
        match op {
            Opcode::GetInitializationInfo => {
                let answered = CalibreInfo::from_init_payload(p);
                let version_ok = p["serverProtocolVersion"].as_u64() == Some(PROTOCOL_VERSION);
                if version_ok && password.is_none() && !str_field(p, "passwordChallenge").is_empty()
                {
                    return Ok(CheckEnd::PasswordNeeded(answered));
                }
                conn.send(
                    Opcode::Ok,
                    init_info(cfg, p, version_ok, password, &Columns::default()),
                )?;
                if !version_ok {
                    return Ok(CheckEnd::ProtocolMismatch);
                }
                info = Some(answered);
            }
            Opcode::GetDeviceInformation => {
                let mut device_info = json!({"device_name": cfg.device_name});
                if let Some(id) = device_uuid {
                    device_info["device_store_uuid"] = json!(id);
                }
                conn.send(
                    Opcode::Ok,
                    json!({"device_info": device_info, "version": VERSION, "device_version": VERSION}),
                )?;
            }
            Opcode::SetCalibreDeviceInfo => {
                // Only the ID Calibre assigned is kept, by the caller.
                if device_uuid.is_none() {
                    assigned = p["device_store_uuid"].as_str().map(str::to_owned);
                }
                conn.send(Opcode::Ok, json!({}))?;
            }
            Opcode::SetCalibreDeviceName => conn.send(Opcode::Ok, json!({}))?,
            Opcode::TotalSpace => {
                conn.send(Opcode::Ok, json!({"total_space_on_device": free_space}))?;
            }
            Opcode::FreeSpace => {
                conn.send(Opcode::Ok, json!({"free_space_on_device": free_space}))?;
            }
            Opcode::SetLibraryInfo => {
                conn.send(Opcode::Ok, json!({}))?;
                columns = Some(readsync::custom_columns(&p["fieldMetadata"]));
            }
            Opcode::GetBookCount => {
                conn.send(
                    Opcode::Ok,
                    json!({"count": 0, "willStream": true, "willScan": true}),
                )?;
                listed = true;
            }
            Opcode::Noop => {
                if p.get("ejecting").and_then(Value::as_bool) == Some(true) {
                    conn.send(Opcode::Ok, json!({}))?;
                    return Ok(match (info, columns) {
                        (Some(info), Some(columns)) => CheckEnd::Done {
                            info,
                            columns,
                            device_uuid: assigned,
                        },
                        _ => CheckEnd::Closed,
                    });
                } else if p.get("priKey").is_some() {
                    conn.send(Opcode::Ok, json!({}))?;
                } else if p.get("count").is_some() {
                    // Announces how many priKey requests follow; no reply.
                } else {
                    // Leave at a quiet keep-alive, without replying (ADR 0022).
                    if listed && last_busy.elapsed() >= quiet {
                        if let (Some(info), Some(columns)) = (info.take(), columns.take()) {
                            return Ok(CheckEnd::Done {
                                info,
                                columns,
                                device_uuid: assigned,
                            });
                        }
                    }
                    conn.send(Opcode::Ok, json!({}))?;
                }
            }
            Opcode::DisplayMessage => {
                conn.send(Opcode::Ok, json!({}))?;
                match (p["messageKind"].as_u64(), info.take()) {
                    (Some(1), Some(info)) => return Ok(CheckEnd::PasswordRejected(info)),
                    (_, i) => {
                        info = i;
                        log::info!("calibre says: {}", p["message"]);
                    }
                }
            }
            Opcode::CalibreBusy => return Ok(CheckEnd::Busy),
            Opcode::SendBooklists | Opcode::SendBookMetadata => {}
            // A job started during the check, say a send: we hold no books
            // and take none, so end here. Calibre reports the job failed.
            Opcode::SendBook | Opcode::DeleteBook | Opcode::GetBookFileSegment => {
                return Err(protocol_error(format!(
                    "calibre started {op} during a check"
                )));
            }
            Opcode::Ok
            | Opcode::Error
            | Opcode::BookDone
            | Opcode::GetBookMetadata
            | Opcode::GetCollections
            | Opcode::UpdateCollections => log::warn!("check: ignoring unexpected {op}"),
        }
    }
}

fn init_info(
    cfg: &DeviceConfig,
    p: &Value,
    version_ok: bool,
    password: Option<&str>,
    named: &Columns,
) -> Value {
    let challenge = str_field(p, "passwordChallenge");
    let password_hash = match password {
        Some(pw) if !challenge.is_empty() => {
            let mut h = sha1_smol::Sha1::new();
            h.update(pw.as_bytes());
            h.update(challenge.as_bytes());
            h.digest().to_string()
        }
        _ => String::new(),
    };
    let ext_lengths: Map<String, Value> = cfg
        .accepted_extensions
        .iter()
        .map(|e| (e.clone(), json!(e.len())))
        .collect();
    let mut info = json!({
        "appName": APP_NAME,
        "acceptedExtensions": cfg.accepted_extensions,
        "cacheUsesLpaths": true,
        "canAcceptLibraryInfo": true,
        "canDeleteMultipleBooks": true,
        "canReceiveBookBinary": true,
        "canSendOkToSendbook": true,
        "canStreamBooks": true,
        "canStreamMetadata": true,
        "canUseCachedMetadata": true,
        "ccVersionNumber": VERSION,
        "coverHeight": cfg.cover_height,
        "deviceKind": cfg.device_kind,
        "deviceName": cfg.device_name,
        "extensionPathLengths": ext_lengths,
        "maxBookContentPacketLen": MAX_BOOK_PACKET,
        "passwordHash": password_hash,
        "useUuidFileNames": false,
        "versionOK": version_ok,
    });
    if let Some(col) = &named.read {
        info["isReadSyncCol"] = json!(col);
    }
    if let Some(col) = &named.last_read {
        info["isReadDateSyncCol"] = json!(col);
    }
    info
}

/// A book-list entry with its read-status fields added.
fn with_fields(mut entry: Value, fields: Option<&Map<String, Value>>) -> Value {
    if let (Some(obj), Some(fields)) = (entry.as_object_mut(), fields) {
        obj.extend(fields.clone());
    }
    entry
}

fn send_book_list<R: BufRead, W: Write>(
    conn: &mut Connection<R, W>,
    lib: &Library,
    p: &Value,
    sync_fields: &[Map<String, Value>],
) -> io::Result<()> {
    let cached = p["willUseCachedMetadata"].as_bool().unwrap_or(false);
    let books = lib.books();
    conn.send(
        Opcode::Ok,
        json!({"count": books.len(), "willStream": true, "willScan": true}),
    )?;
    for (index, book) in books.iter().enumerate() {
        if cached {
            let lpath = str_field(book, "lpath");
            let extension = lpath
                .rsplit_once('.')
                .map(|(_, e)| e.to_string())
                .unwrap_or_default();
            let entry = json!({
                "priKey": index,
                "uuid": book.get("uuid").cloned().unwrap_or(Value::Null),
                "lpath": lpath,
                "last_modified": book.get("last_modified").cloned().unwrap_or(json!("None")),
                "extension": extension,
            });
            conn.send(Opcode::Ok, with_fields(entry, sync_fields.get(index)))?;
        } else {
            let book = lib.calibre_metadata(index).unwrap_or_default();
            conn.send(Opcode::Ok, with_fields(book, sync_fields.get(index)))?;
        }
    }
    Ok(())
}

fn receive_book<R: BufRead, W: Write>(
    conn: &mut Connection<R, W>,
    lib: &mut Library,
    cfg: &DeviceConfig,
    p: &Value,
    folder: &str,
) -> io::Result<()> {
    let length = p["length"]
        .as_u64()
        .ok_or_else(|| protocol_error("SEND_BOOK without length"))?;
    let wants_ok = p["wantsSendOkToSendbook"].as_bool().unwrap_or(false);
    // Calibre takes the lpath in our OK as the book's, so the book can go
    // elsewhere than Calibre's template says, but only when it wants one.
    let lpath = if wants_ok {
        place(lib, p, folder)
    } else {
        str_field(p, "lpath")
    };
    let progress = format!(
        "{}/{}",
        p["thisBook"].as_u64().unwrap_or(0) + 1,
        p["totalBooks"]
    );

    let prepared = lpath::validate(&lpath, Some(&cfg.accepted_extensions))
        .map_err(|e| e.to_string())
        .and_then(|rel| {
            if p["metadata"].is_object() {
                Ok(rel)
            } else {
                Err("missing book metadata".to_string())
            }
        })
        .and_then(|rel| lib.begin_incoming(&rel, length).map_err(|e| e.to_string()));

    let mut incoming = match prepared {
        Ok(incoming) => incoming,
        Err(reason) => {
            log::warn!("refusing book {progress} {lpath:?}: {reason}");
            if wants_ok {
                // Calibre sends no book bytes after an ERROR reply.
                conn.send(
                    Opcode::Error,
                    json!({"message": format!("Cannot store {lpath}: {reason}")}),
                )?;
            } else {
                conn.recv_raw(length, &mut io::sink())?;
            }
            return Ok(());
        }
    };
    if wants_ok {
        conn.send(Opcode::Ok, json!({"lpath": lpath}))?;
    }
    conn.recv_raw(length, incoming.writer())?;
    match lib.commit_incoming(incoming, p["metadata"].clone()) {
        Ok(()) => log::info!("received book {progress} {lpath} ({length} bytes)"),
        // Calibre expects no reply here; it will see the book missing from
        // the next book list.
        Err(e) => log::error!("failed to store {lpath}: {e}"),
    }
    Ok(())
}

/// Where a book Calibre sends goes (ADR 0023): over the book we hold with
/// the same uuid and format, wherever that is, or into the books folder.
fn place(lib: &Library, p: &Value, folder: &str) -> String {
    let lpath = str_field(p, "lpath");
    if let Some(held) = p["metadata"]["uuid"]
        .as_str()
        .and_then(|uuid| lib.lpath_of(uuid, &lpath))
    {
        return held;
    }
    match folder.trim_matches('/') {
        "" => lpath,
        folder => format!("{folder}/{lpath}"),
    }
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn version_string(v: &Value) -> String {
    match v.as_array() {
        Some(parts) => parts
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join("."),
        None => v.as_str().unwrap_or("unknown").to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{read_message, Message};
    use std::io::Cursor;

    /// Builds the byte stream a scripted Calibre would send.
    struct Script(Vec<u8>);

    impl Script {
        fn new() -> Self {
            Script(Vec::new())
        }
        fn msg(mut self, op: Opcode, payload: Value) -> Self {
            crate::protocol::write_message(&mut self.0, op, &payload).unwrap();
            self
        }
        fn raw(mut self, bytes: &[u8]) -> Self {
            self.0.extend_from_slice(bytes);
            self
        }
        fn handshake(self) -> Self {
            self.msg(
                Opcode::GetInitializationInfo,
                json!({
                    "serverProtocolVersion": 1,
                    "passwordChallenge": "",
                    "currentLibraryName": "Calibre Library",
                    "currentLibraryUUID": "lib-1",
                    "calibre_version": [9, 16, 0],
                    "validExtensions": ["epub", "azw3"],
                }),
            )
        }
    }

    struct Outcome {
        end: SessionEnd,
        replies: Vec<(Opcode, Value)>,
        raw_after: Vec<u8>,
    }

    fn run_script(lib: &mut Library, cfg: &DeviceConfig, script: Script) -> Outcome {
        run_script_with_password(lib, cfg, script, None)
    }

    /// Admits library "lib-1" with `password`.
    fn run_script_with_password(
        lib: &mut Library,
        cfg: &DeviceConfig,
        script: Script,
        password: Option<&str>,
    ) -> Outcome {
        run_script_with(lib, cfg, script, password, None, &mut Map::new())
    }

    /// Admits library "lib-1" with `password` and sync `columns`, and
    /// reconnects at the first keep-alive. `known` stands in for the
    /// columns kept in `config.json`: the session reads and updates it.
    fn run_script_with(
        lib: &mut Library,
        cfg: &DeviceConfig,
        script: Script,
        password: Option<&str>,
        columns: Option<Columns>,
        known: &mut Map<String, Value>,
    ) -> Outcome {
        run_script_quiet(lib, cfg, script, password, columns, known, Duration::ZERO)
    }

    struct TestAdmit<'a> {
        password: Option<&'a str>,
        columns: Option<Columns>,
        known: &'a mut Map<String, Value>,
        folder: &'a str,
    }

    impl Admit for TestAdmit<'_> {
        fn admit(&mut self, info: &CalibreInfo) -> Admission {
            if info.library_uuid == "lib-1" {
                Admission::Accept {
                    password: self.password.map(str::to_owned),
                    columns: self.columns.clone(),
                    known_columns: self.known.clone(),
                }
            } else {
                Admission::Reject
            }
        }

        fn columns_seen(&mut self, columns: &Map<String, Value>) -> io::Result<()> {
            *self.known = columns.clone();
            Ok(())
        }

        fn books_folder(&mut self) -> String {
            self.folder.to_owned()
        }
    }

    /// [`run_script_with`], waiting `quiet` before a reconnect.
    fn run_script_quiet(
        lib: &mut Library,
        cfg: &DeviceConfig,
        script: Script,
        password: Option<&str>,
        columns: Option<Columns>,
        known: &mut Map<String, Value>,
        quiet: Duration,
    ) -> Outcome {
        run_script_full(lib, cfg, script, password, columns, known, quiet, "")
    }

    /// [`run_script`], with new books going into `folder` (ADR 0023).
    fn run_script_in(lib: &mut Library, script: Script, folder: &str) -> Outcome {
        let cfg = DeviceConfig::default();
        let mut known = Map::new();
        run_script_full(
            lib,
            &cfg,
            script,
            None,
            None,
            &mut known,
            Duration::ZERO,
            folder,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn run_script_full(
        lib: &mut Library,
        cfg: &DeviceConfig,
        script: Script,
        password: Option<&str>,
        columns: Option<Columns>,
        known: &mut Map<String, Value>,
        quiet: Duration,
        folder: &str,
    ) -> Outcome {
        let mut out = Vec::new();
        let end = {
            let mut conn = Connection::new(Cursor::new(script.0), &mut out);
            let mut admit = TestAdmit {
                password,
                columns,
                known,
                folder,
            };
            run_with_quiet(&mut conn, lib, cfg, &mut admit, quiet).unwrap()
        };
        // Decode replies; anything after the last frame is raw file data.
        let mut cur = Cursor::new(out);
        let mut replies = Vec::new();
        loop {
            let pos = cur.position();
            match read_message(&mut cur) {
                Ok(Some(Message { code, payload })) => {
                    replies.push((Opcode::from_code(code).unwrap(), payload))
                }
                _ => {
                    cur.set_position(pos);
                    break;
                }
            }
        }
        let rest = cur.get_ref()[cur.position() as usize..].to_vec();
        Outcome {
            end,
            replies,
            raw_after: rest,
        }
    }

    fn setup() -> (tempfile::TempDir, Library) {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(&tmp.path().join("books"), &tmp.path().join("state")).unwrap();
        (tmp, lib)
    }

    fn send_book(script: Script, lpath: &str, data: &[u8]) -> Script {
        script
            .msg(
                Opcode::SendBook,
                json!({
                    "lpath": lpath, "length": data.len(), "thisBook": 0, "totalBooks": 1,
                    "willStreamBooks": true, "willStreamBinary": true,
                    "wantsSendOkToSendbook": true, "canSupportLpathChanges": true,
                    "metadata": {"uuid": "book-1", "title": "Rogue Protocol", "lpath": lpath,
                                  "last_modified": "2026-10-15T00:00:00+00:00",
                                  "thumbnail": [331, 500, "AAAA"]},
                }),
            )
            .raw(data)
    }

    #[test]
    fn handshake_advertises_capabilities_and_ends_on_eject() {
        let (_tmp, mut lib) = setup();
        let script = Script::new()
            .handshake()
            .msg(Opcode::GetDeviceInformation, json!({}))
            .msg(
                Opcode::SetCalibreDeviceInfo,
                json!({"device_store_uuid": "dev-1", "device_name": "Kindle"}),
            )
            .msg(Opcode::FreeSpace, json!({}))
            .msg(Opcode::Noop, json!({}))
            .msg(Opcode::Noop, json!({"ejecting": true}));
        let o = run_script(&mut lib, &DeviceConfig::default(), script);
        assert_eq!(o.end, SessionEnd::Ejected);
        let (op, init) = &o.replies[0];
        assert_eq!(*op, Opcode::Ok);
        for key in [
            "canStreamBooks",
            "canStreamMetadata",
            "canReceiveBookBinary",
            "canDeleteMultipleBooks",
            "versionOK",
        ] {
            assert_eq!(init[key], true, "{key}");
        }
        assert_eq!(init["acceptedExtensions"][0], "azw3");
        assert_eq!(init["passwordHash"], "");
        assert!(init["maxBookContentPacketLen"].as_u64().unwrap() > 0);
        assert_eq!(o.replies[1].1["device_info"]["device_name"], "Kindle");
        assert_eq!(lib.driveinfo()["device_store_uuid"], "dev-1");
        assert!(o.replies[3].1["free_space_on_device"].as_u64().unwrap() > 0);
        assert_eq!(o.replies.len(), 6);
    }

    fn library_info(columns: Value) -> (Opcode, Value) {
        (
            Opcode::SetLibraryInfo,
            json!({"libraryName": "Calibre Library", "libraryUuid": "lib-1",
                   "fieldMetadata": columns, "otherInfo": {}}),
        )
    }

    fn sync_columns() -> Value {
        json!({
            "#kindle_read": {"is_custom": true, "datatype": "bool"},
            "#kindle_last_read": {"is_custom": true, "datatype": "datetime"},
            "title": {"is_custom": false, "datatype": "text"},
        })
    }

    #[test]
    fn read_sync_columns_appearing_reconnect_then_get_named() {
        let (_tmp, mut lib) = setup();
        let mut known = Map::new();
        let cfg = DeviceConfig::default();
        let (op, info) = library_info(sync_columns());
        let script = || {
            Script::new()
                .handshake()
                .msg(op, info.clone())
                .msg(Opcode::Noop, json!({}))
                .msg(Opcode::Noop, json!({"ejecting": true}))
        };

        // Calibre hasn't been idle long enough: no reconnect yet, so it
        // never sees us drop between two of its jobs.
        let o = run_script_quiet(
            &mut lib,
            &cfg,
            script(),
            None,
            Some(Columns::defaults()),
            &mut known,
            Duration::from_secs(3600),
        );
        assert_eq!(o.end, SessionEnd::Ejected);
        known.clear();

        // First connection: not seen yet, so not named; seeing them ends it
        // at the keep-alive, without answering it (ADR 0022).
        let o = run_script_with(
            &mut lib,
            &cfg,
            script(),
            None,
            Some(Columns::defaults()),
            &mut known,
        );
        assert_eq!(o.end, SessionEnd::ColumnsChanged);
        assert!(o.replies[0].1.get("isReadSyncCol").is_none());
        // Init and the library info only.
        assert_eq!(o.replies.len(), 2);
        assert_eq!(known["#kindle_read"]["datatype"], "bool");

        // The reconnect names them, and the same list doesn't end it again.
        let o = run_script_with(
            &mut lib,
            &cfg,
            script(),
            None,
            Some(Columns::defaults()),
            &mut known,
        );
        assert_eq!(o.end, SessionEnd::Ejected);
        assert_eq!(o.replies[0].1["isReadSyncCol"], "#kindle_read");
        assert_eq!(o.replies[0].1["isReadDateSyncCol"], "#kindle_last_read");

        // A library that isn't writable never names them or reconnects.
        let o = run_script_with(&mut lib, &cfg, script(), None, None, &mut known);
        assert_eq!(o.end, SessionEnd::Ejected);
        assert!(o.replies[0].1.get("isReadSyncCol").is_none());
    }

    #[test]
    fn leave_now_ends_at_the_next_keep_alive_even_when_busy() {
        struct Leaving;
        impl Admit for Leaving {
            fn admit(&mut self, _: &CalibreInfo) -> Admission {
                Admission::Accept {
                    password: None,
                    columns: None,
                    known_columns: Map::new(),
                }
            }
            fn leave_now(&mut self) -> bool {
                true
            }
        }
        let (_tmp, mut lib) = setup();
        let script = Script::new()
            .handshake()
            .msg(Opcode::FreeSpace, json!({}))
            .msg(Opcode::Noop, json!({"count": 0}))
            .msg(Opcode::Noop, json!({}))
            .msg(Opcode::FreeSpace, json!({}));
        let mut out = Vec::new();
        let end = {
            let mut conn = Connection::new(Cursor::new(script.0), &mut out);
            // Calibre was busy a moment ago; leaving doesn't wait for quiet.
            run_with_quiet(
                &mut conn,
                &mut lib,
                &DeviceConfig::default(),
                &mut Leaving,
                Duration::from_secs(3600),
            )
            .unwrap()
        };
        assert_eq!(end, SessionEnd::Left);
        // Init and free space; the keep-alive isn't answered, and nothing
        // after it is read.
        let mut cur = Cursor::new(out);
        let mut replies = 0;
        while let Ok(Some(_)) = read_message(&mut cur) {
            replies += 1;
        }
        assert_eq!(replies, 2);
    }

    #[test]
    fn a_library_switch_in_calibre_ends_the_session_quietly() {
        let (_tmp, mut lib) = setup();
        let cfg = DeviceConfig::default();
        let (op, info) = library_info(sync_columns());
        // Calibre switches to another library: it keeps the device and
        // sends only the new library's info.
        let other = json!({"libraryName": "Other Library", "libraryUuid": "lib-2",
                           "fieldMetadata": {"#x": {"is_custom": true, "datatype": "bool"}},
                           "otherInfo": {}});
        let script = || {
            Script::new()
                .handshake()
                .msg(op, info.clone())
                .msg(Opcode::SetLibraryInfo, other.clone())
                .msg(Opcode::Noop, json!({}))
                .msg(Opcode::Noop, json!({"ejecting": true}))
        };
        let mut known = Map::new();
        let o = run_script_with(&mut lib, &cfg, script(), None, None, &mut known);
        assert!(
            matches!(o.end, SessionEnd::LibraryChanged(ref i) if i.library_uuid == "lib-2"
                && i.library_name == "Other Library"),
            "{:?}",
            o.end
        );
        // Every request answered but the keep-alive, where it leaves
        // without a reply (ADR 0022).
        assert_eq!(o.replies.len(), 3);
        // The other library's columns aren't kept as the linked one's.
        assert!(known.contains_key("#kindle_read"));
        assert!(!known.contains_key("#x"));

        // Not before Calibre is quiet.
        let o = run_script_quiet(
            &mut lib,
            &cfg,
            script(),
            None,
            None,
            &mut known,
            Duration::from_secs(3600),
        );
        assert_eq!(o.end, SessionEnd::Ejected);
    }

    #[test]
    fn read_sync_stops_quietly_when_columns_are_removed() {
        let (_tmp, mut lib) = setup();
        let cfg = DeviceConfig::default();
        let mut known = readsync::custom_columns(&sync_columns());
        // The user deleted one column; the other changed type.
        let (op, info) = library_info(json!({
            "#kindle_last_read": {"is_custom": true, "datatype": "text"},
        }));
        let script = || {
            Script::new()
                .handshake()
                .msg(op, info.clone())
                .msg(Opcode::GetBookCount, json!({"willUseCachedMetadata": true}))
                .msg(Opcode::Noop, json!({}))
                .msg(Opcode::Noop, json!({"ejecting": true}))
        };
        let o = run_script_with(
            &mut lib,
            &cfg,
            script(),
            None,
            Some(Columns::defaults()),
            &mut known,
        );
        assert_eq!(
            o.replies[0].1["isReadSyncCol"], "#kindle_read",
            "named from the old record"
        );
        // The session carries on (Calibre skips its sync, as a column is
        // missing), then ends at the keep-alive without answering it: the
        // last reply is the book count.
        assert_eq!(o.end, SessionEnd::ColumnsChanged);
        assert_eq!(o.replies.last().unwrap().1["count"], 0);

        let o = run_script_with(
            &mut lib,
            &cfg,
            script(),
            None,
            Some(Columns::defaults()),
            &mut known,
        );
        assert_eq!(o.end, SessionEnd::Ejected);
        assert!(o.replies[0].1.get("isReadSyncCol").is_none());
        assert!(o.replies[0].1.get("isReadDateSyncCol").is_none());
        // No read fields in the book list.
        assert!(o.replies[2].1.get("count").is_some());
        assert!(o.replies.iter().all(|(_, v)| v.get("_is_read_").is_none()));
    }

    #[test]
    fn book_list_carries_read_sync_fields_when_named() {
        let (_tmp, mut lib) = setup();
        let cfg = DeviceConfig::default();
        let o = run_script(
            &mut lib,
            &cfg,
            send_book(Script::new().handshake(), "a.azw3", b"x"),
        );
        assert_eq!(o.end, SessionEnd::Closed);
        let mut known = readsync::custom_columns(&sync_columns());
        let (op, info) = library_info(sync_columns());
        let script = Script::new()
            .handshake()
            .msg(op, info)
            .msg(Opcode::GetBookCount, json!({"willUseCachedMetadata": true}))
            .msg(Opcode::Noop, json!({"priKey": 0}));
        let o = run_script_with(
            &mut lib,
            &cfg,
            script,
            None,
            Some(Columns::defaults()),
            &mut known,
        );
        // After the handshake and library info: OK for the count, then the
        // cached entry, then the full metadata.
        let entry = &o.replies[3].1;
        assert_eq!(entry["lpath"], "a.azw3");
        // No catalog and no Calibre value yet: a first sync, with no value.
        assert_eq!(entry["_sync_type_"], 2);
        assert_eq!(entry["_is_read_"], Value::Null);
        assert!(entry.as_object().unwrap().contains_key("_last_read_date_"));
        let full = &o.replies[4].1;
        assert_eq!(full["title"], "Rogue Protocol");
        assert_eq!(full["_sync_type_"], 2);
        assert!(full.get("_kws").is_none());
    }

    #[test]
    fn password_hash_is_sha1_of_password_and_challenge() {
        let (_tmp, mut lib) = setup();
        let cfg = DeviceConfig::default();
        let script = Script::new().msg(
            Opcode::GetInitializationInfo,
            json!({"serverProtocolVersion": 1, "passwordChallenge": "2026-10-15T00:00:00",
                   "currentLibraryUUID": "lib-1", "calibre_version": [9, 16, 0]}),
        );
        let o = run_script_with_password(&mut lib, &cfg, script, Some("pw"));
        let mut h = sha1_smol::Sha1::new();
        h.update(b"pw2026-10-15T00:00:00");
        assert_eq!(o.replies[0].1["passwordHash"], h.digest().to_string());
        assert_eq!(o.end, SessionEnd::Closed);
    }

    #[test]
    fn rejected_library_gets_no_reply() {
        let (_tmp, mut lib) = setup();
        let script = Script::new().msg(
            Opcode::GetInitializationInfo,
            json!({"serverProtocolVersion": 1, "currentLibraryUUID": "someone-else",
                   "currentLibraryName": "Other", "calibre_version": [9, 16, 0]}),
        );
        let o = run_script(&mut lib, &DeviceConfig::default(), script);
        assert!(
            matches!(o.end, SessionEnd::LibraryRejected(ref i) if i.library_uuid == "someone-else")
        );
        assert!(o.replies.is_empty());
    }

    #[test]
    fn protocol_version_mismatch_is_reported() {
        let (_tmp, mut lib) = setup();
        let script = Script::new().msg(
            Opcode::GetInitializationInfo,
            json!({"serverProtocolVersion": 2, "currentLibraryUUID": "lib-1"}),
        );
        let o = run_script(&mut lib, &DeviceConfig::default(), script);
        assert_eq!(o.end, SessionEnd::ProtocolMismatch);
        assert_eq!(o.replies[0].1["versionOK"], false);
    }

    #[test]
    fn receives_a_book_and_lists_it_from_cache() {
        let (_tmp, mut lib) = setup();
        let script = send_book(
            Script::new().handshake(),
            "Martha Wells/Rogue Protocol.azw3",
            b"BOOKDATA",
        )
        .msg(
            Opcode::GetBookCount,
            json!({"willUseCachedMetadata": true, "canStream": true}),
        )
        .msg(Opcode::Noop, json!({"count": 1}))
        .msg(Opcode::Noop, json!({"priKey": 0}));
        let o = run_script(&mut lib, &DeviceConfig::default(), script);
        assert_eq!(o.end, SessionEnd::Closed);
        assert_eq!(
            o.replies[1],
            (
                Opcode::Ok,
                json!({"lpath": "Martha Wells/Rogue Protocol.azw3"})
            )
        );
        assert_eq!(
            std::fs::read(lib.books_dir().join("Martha Wells/Rogue Protocol.azw3")).unwrap(),
            b"BOOKDATA"
        );
        assert_eq!(o.replies[2].1["count"], 1);
        let rec = &o.replies[3].1;
        assert_eq!(rec["priKey"], 0);
        assert_eq!(rec["uuid"], "book-1");
        assert_eq!(rec["lpath"], "Martha Wells/Rogue Protocol.azw3");
        assert_eq!(rec["last_modified"], "2026-10-15T00:00:00+00:00");
        assert_eq!(rec["extension"], "azw3");
        // NOOP{count} gets no reply; NOOP{priKey} gets the full record.
        assert_eq!(o.replies.len(), 5);
        assert_eq!(o.replies[4].1["title"], "Rogue Protocol");
        assert_eq!(o.replies[4].1["thumbnail"], Value::Null);
    }

    #[test]
    fn new_books_go_into_the_books_folder_and_updates_stay_put() {
        let (_tmp, mut lib) = setup();
        let books = lib.books_dir().to_path_buf();
        let send = |lpath: &str, data: &[u8]| send_book(Script::new().handshake(), lpath, data);

        // A new book goes into the folder, and Calibre is told its lpath.
        let o = run_script_in(&mut lib, send("Rogue Protocol.azw3", b"ONE"), "Calibre/SF");
        assert_eq!(
            o.replies[1].1,
            json!({"lpath": "Calibre/SF/Rogue Protocol.azw3"})
        );
        assert_eq!(
            std::fs::read(books.join("Calibre/SF/Rogue Protocol.azw3")).unwrap(),
            b"ONE"
        );
        assert!(lib.book("Calibre/SF/Rogue Protocol.azw3").is_some());

        // The folder changes. An update to the same book (same uuid and
        // format) stays where it is, even under a new name from Calibre.
        let o = run_script_in(&mut lib, send("Rogue Protocol (new).azw3", b"TWO"), "Other");
        assert_eq!(
            o.replies[1].1,
            json!({"lpath": "Calibre/SF/Rogue Protocol.azw3"})
        );
        assert_eq!(
            std::fs::read(books.join("Calibre/SF/Rogue Protocol.azw3")).unwrap(),
            b"TWO"
        );
        assert!(!books.join("Other").exists());
        assert_eq!(lib.books().len(), 1);

        // Another format of it is a new file, in the current folder.
        let o = run_script_in(&mut lib, send("Rogue Protocol.pdf", b"PDF"), "Other");
        assert_eq!(o.replies[1].1, json!({"lpath": "Other/Rogue Protocol.pdf"}));
        assert_eq!(lib.books().len(), 2);

        // No folder: the books folder itself, as Calibre names it.
        let o = run_script_in(&mut lib, send("Rogue Protocol.txt", b"TXT"), "");
        assert_eq!(o.replies[1].1, json!({"lpath": "Rogue Protocol.txt"}));
    }

    #[test]
    fn refused_book_gets_error_and_session_continues() {
        let (_tmp, mut lib) = setup();
        // No raw bytes follow an ERROR reply, so the next frame comes next.
        let script = Script::new()
            .handshake()
            .msg(
                Opcode::SendBook,
                json!({"lpath": "../escape.azw3", "length": 4, "wantsSendOkToSendbook": true,
                       "metadata": {}, "thisBook": 0, "totalBooks": 1}),
            )
            .msg(Opcode::Noop, json!({}));
        let o = run_script(&mut lib, &DeviceConfig::default(), script);
        assert_eq!(o.replies[1].0, Opcode::Error);
        assert!(o.replies[1].1["message"]
            .as_str()
            .unwrap()
            .contains("escape"));
        assert_eq!(o.replies[2], (Opcode::Ok, json!({})));
        assert!(lib.books().is_empty());
    }

    #[test]
    fn epub_is_refused() {
        let (_tmp, mut lib) = setup();
        let o = run_script(
            &mut lib,
            &DeviceConfig::default(),
            send_book(Script::new().handshake(), "b.epub", b""),
        );
        assert_eq!(o.replies[1].0, Opcode::Error);
    }

    #[test]
    fn deletes_books_and_answers_once_per_lpath() {
        let (_tmp, mut lib) = setup();
        let script = send_book(Script::new().handshake(), "a.azw3", b"x").msg(
            Opcode::DeleteBook,
            json!({"lpaths": ["a.azw3", "never-had.azw3"]}),
        );
        let o = run_script(&mut lib, &DeviceConfig::default(), script);
        assert_eq!(o.replies[2], (Opcode::Ok, json!({})));
        assert_eq!(o.replies[3], (Opcode::Ok, json!({"uuid": "book-1"})));
        assert_eq!(o.replies[4], (Opcode::Ok, json!({"uuid": ""})));
        assert!(!lib.books_dir().join("a.azw3").exists());
    }

    #[test]
    fn reports_books_sent_over_usb_once_per_session() {
        let (_tmp, mut lib) = setup();
        let uuid = "cb5d2673-dbcf-4a0c-b9a6-90de29a24177";
        let book = crate::kindle::tests::fake_mobi(&[
            (112, format!("calibre:{uuid}").as_bytes()),
            (503, b"Example Book"),
        ]);
        std::fs::write(lib.books_dir().join("Example.azw3"), book).unwrap();
        let count = json!({"willUseCachedMetadata": true});
        let script = Script::new()
            .handshake()
            .msg(Opcode::GetBookCount, count.clone())
            .msg(Opcode::Noop, json!({"priKey": 0}));
        let o = run_script(&mut lib, &DeviceConfig::default(), script);
        assert_eq!(o.replies[1].1["count"], 1);
        assert_eq!(o.replies[2].1["uuid"], uuid);
        assert_eq!(o.replies[2].1["lpath"], "Example.azw3");
        // On a cache miss Calibre gets the record we built from the file.
        assert_eq!(o.replies[3].1["title"], "Example Book");
        assert_eq!(o.replies[3].1["authors"], json!(["Unknown"]));
    }

    #[test]
    fn sends_held_books_back_to_calibre() {
        let (_tmp, mut lib) = setup();
        let script = send_book(Script::new().handshake(), "a.azw3", b"CONTENT").msg(
            Opcode::GetBookFileSegment,
            json!({"lpath": "a.azw3", "position": 0}),
        );
        let o = run_script(&mut lib, &DeviceConfig::default(), script);
        assert_eq!(o.replies[2], (Opcode::Ok, json!({"fileLength": 7})));
        assert_eq!(o.raw_after, b"CONTENT");
    }

    /// Runs a check against a script, with no quiet wait.
    fn run_check(
        script: Script,
        id: Option<&str>,
        password: Option<&str>,
    ) -> (CheckEnd, Vec<(Opcode, Value)>) {
        let mut out = Vec::new();
        let end = {
            let mut conn = Connection::new(Cursor::new(script.0), &mut out);
            check(
                &mut conn,
                &DeviceConfig::default(),
                id,
                password,
                1000,
                Duration::ZERO,
            )
            .unwrap()
        };
        let mut cur = Cursor::new(out);
        let mut replies = Vec::new();
        while let Ok(Some(m)) = read_message(&mut cur) {
            replies.push((m.opcode().unwrap(), m.payload));
        }
        (end, replies)
    }

    /// A full check as Calibre runs it, up to its first keep-alive.
    fn check_script(device_uuid: &str) -> Script {
        let (op, info) = library_info(json!({
            "#kindle_read": {"is_custom": true, "datatype": "bool", "name": "Kindle read"},
        }));
        Script::new()
            .handshake()
            .msg(Opcode::GetDeviceInformation, json!({}))
            .msg(
                Opcode::SetCalibreDeviceInfo,
                json!({"device_store_uuid": device_uuid, "device_name": "Kindle",
                       "last_library_uuid": "lib-1"}),
            )
            .msg(Opcode::FreeSpace, json!({}))
            .msg(op, info)
            .msg(Opcode::GetBookCount, json!({"willUseCachedMetadata": true}))
            .msg(Opcode::Noop, json!({"count": 0}))
            .msg(Opcode::Noop, json!({}))
            .msg(Opcode::Noop, json!({}))
    }

    #[test]
    fn a_check_learns_the_columns_lists_no_books_and_leaves_at_a_keep_alive() {
        let (end, replies) = run_check(check_script("assigned-1"), None, None);
        let CheckEnd::Done {
            info,
            columns,
            device_uuid,
        } = end
        else {
            panic!("{end:?}");
        };
        assert_eq!(info.library_uuid, "lib-1");
        assert_eq!(columns["#kindle_read"]["name"], "Kindle read");
        // No ID was given, so Calibre's is returned for next time.
        assert_eq!(device_uuid.as_deref(), Some("assigned-1"));
        // Init, device info, driveinfo, free space, library info and book
        // count; it leaves at the first keep-alive without answering it.
        assert_eq!(replies.len(), 6);
        assert!(replies[0].1.get("isReadSyncCol").is_none());
        assert!(replies[1].1["device_info"]
            .get("device_store_uuid")
            .is_none());
        assert_eq!(replies[1].1["device_info"]["device_name"], "Kindle");
        assert_eq!(replies[5].1["count"], 0);

        // With an ID, it's the one sent, and Calibre's echo isn't returned.
        let (end, replies) = run_check(check_script("check-1"), Some("check-1"), None);
        assert!(matches!(
            end,
            CheckEnd::Done {
                device_uuid: None,
                ..
            }
        ));
        assert_eq!(replies[1].1["device_info"]["device_store_uuid"], "check-1");
    }

    #[test]
    fn a_check_reports_passwords_lockouts_and_busy() {
        let challenge = || {
            Script::new().msg(
                Opcode::GetInitializationInfo,
                json!({"serverProtocolVersion": 1, "passwordChallenge": "2026-10-18T00:00:00",
                       "currentLibraryUUID": "lib-1", "currentLibraryName": "Calibre Library"}),
            )
        };
        let (end, replies) = run_check(challenge(), None, None);
        assert!(matches!(end, CheckEnd::PasswordNeeded(ref i) if i.library_uuid == "lib-1"));
        assert!(replies.is_empty(), "nothing answered without a password");

        let (end, replies) = run_check(
            challenge().msg(Opcode::DisplayMessage, json!({"messageKind": 1})),
            None,
            Some("wrong"),
        );
        assert!(matches!(end, CheckEnd::PasswordRejected(_)));
        assert_ne!(replies[0].1["passwordHash"], "");

        assert_eq!(
            run_check(Script::new(), None, None).0,
            CheckEnd::ClosedAtOnce
        );
        assert_eq!(
            run_check(Script::new().handshake(), None, None).0,
            CheckEnd::Closed
        );
        assert_eq!(
            run_check(
                Script::new().msg(Opcode::CalibreBusy, json!({})),
                None,
                None
            )
            .0,
            CheckEnd::Busy
        );
    }

    #[test]
    fn busy_and_password_errors_end_the_session() {
        let (_tmp, mut lib) = setup();
        let o = run_script(
            &mut lib,
            &DeviceConfig::default(),
            Script::new().msg(Opcode::CalibreBusy, json!({})),
        );
        assert_eq!(o.end, SessionEnd::Busy);
        let o = run_script(
            &mut lib,
            &DeviceConfig::default(),
            Script::new()
                .handshake()
                .msg(Opcode::DisplayMessage, json!({"messageKind": 1})),
        );
        assert_eq!(o.end, SessionEnd::PasswordRejected);
        // Closed before Calibre said anything: its lockout (ADR 0020).
        let o = run_script(&mut lib, &DeviceConfig::default(), Script::new());
        assert_eq!(o.end, SessionEnd::ClosedAtOnce);
        let o = run_script(
            &mut lib,
            &DeviceConfig::default(),
            Script::new().handshake(),
        );
        assert_eq!(o.end, SessionEnd::Closed);
    }

    #[test]
    fn messages_from_calibre_get_a_reply() {
        let (_tmp, mut lib) = setup();
        let o = run_script(
            &mut lib,
            &DeviceConfig::default(),
            Script::new()
                .handshake()
                .msg(
                    Opcode::DisplayMessage,
                    json!({"messageKind": 3, "message": "The read sync column #x is not in calibre's library"}),
                )
                .msg(Opcode::Noop, json!({"ejecting": true})),
        );
        assert_eq!(o.end, SessionEnd::Ejected);
        assert_eq!(o.replies[1], (Opcode::Ok, json!({})));
    }

    #[test]
    fn metadata_updates_are_stored() {
        let (_tmp, mut lib) = setup();
        let script = send_book(Script::new().handshake(), "a.azw3", b"x")
            .msg(
                Opcode::SendBooklists,
                json!({"count": 1, "collections": {"Murderbot": ["a.azw3"]}}),
            )
            .msg(
                Opcode::SendBookMetadata,
                json!({"index": 0, "count": 1, "data": {"lpath": "a.azw3", "title": "Renamed"}}),
            );
        let o = run_script(&mut lib, &DeviceConfig::default(), script);
        assert_eq!(o.replies.len(), 2, "booklists and metadata get no replies");
        assert_eq!(lib.book("a.azw3").unwrap()["title"], "Renamed");
    }
}
