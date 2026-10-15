//! The device side of one Calibre wireless session.
//!
//! Calibre drives the conversation: it sends a request, we answer. The order
//! and reply shapes mirror what Calibre's smart device driver expects; see
//! `docs/research/calibre-wireless-protocol.md` for the walkthrough.

use std::io::{self, BufRead, Write};

use serde_json::{json, Map, Value};

use crate::library::Library;
use crate::lpath;
use crate::protocol::{Connection, Opcode};

pub const APP_NAME: &str = "Kindle Wireless Sync";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOL_VERSION: u64 = 1;
/// Chunk size Calibre uses when streaming book bytes to us.
const MAX_BOOK_PACKET: u64 = 64 * 1024;

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
#[derive(Debug, Clone, PartialEq)]
pub struct CalibreInfo {
    pub library_uuid: String,
    pub library_name: String,
    pub calibre_version: String,
}

impl CalibreInfo {
    /// Reads the library identity from a `GET_INITIALIZATION_INFO` payload.
    pub fn from_init_payload(p: &Value) -> CalibreInfo {
        CalibreInfo {
            library_uuid: str_field(p, "currentLibraryUUID"),
            library_name: str_field(p, "currentLibraryName"),
            calibre_version: version_string(&p["calibre_version"]),
        }
    }
}

/// The answer to "may we talk to this library?", with the password it needs.
#[derive(Debug, Clone, PartialEq)]
pub enum Admission {
    Reject,
    Accept { password: Option<String> },
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
    admit: &mut dyn FnMut(&CalibreInfo) -> Admission,
) -> io::Result<SessionEnd> {
    let mut adopted = false;
    loop {
        let Some(msg) = conn.recv()? else {
            return Ok(SessionEnd::Closed);
        };
        let Some(op) = msg.opcode() else {
            log::warn!("ignoring unknown opcode {}", msg.code);
            continue;
        };
        let p = &msg.payload;
        match op {
            Opcode::GetInitializationInfo => {
                let info = CalibreInfo::from_init_payload(p);
                let version_ok = p["serverProtocolVersion"].as_u64() == Some(PROTOCOL_VERSION);
                let admission = if version_ok {
                    admit(&info)
                } else {
                    Admission::Accept { password: None }
                };
                let Admission::Accept { password } = admission else {
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
                conn.send(
                    Opcode::Ok,
                    init_info(cfg, p, version_ok, password.as_deref()),
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
            Opcode::SetLibraryInfo => conn.send(Opcode::Ok, json!({}))?,
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
                send_book_list(conn, lib, p)?
            }
            Opcode::Noop => {
                if p.get("ejecting").and_then(Value::as_bool) == Some(true) {
                    conn.send(Opcode::Ok, json!({}))?;
                    return Ok(SessionEnd::Ejected);
                } else if let Some(index) = p.get("priKey").and_then(Value::as_u64) {
                    // Calibre's cache missed this book; send full metadata.
                    let book = lib
                        .calibre_metadata(index as usize)
                        .unwrap_or_else(|| json!({}));
                    conn.send(Opcode::Ok, book)?;
                } else if p.get("count").is_some() {
                    // Announces how many priKey requests follow; no reply.
                } else {
                    conn.send(Opcode::Ok, json!({}))?;
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
            Opcode::SendBook => receive_book(conn, lib, cfg, p)?,
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
            Opcode::DisplayMessage => match p["messageKind"].as_u64() {
                Some(1) => return Ok(SessionEnd::PasswordRejected),
                _ => log::info!("calibre says: {}", p["message"]),
            },
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

fn init_info(cfg: &DeviceConfig, p: &Value, version_ok: bool, password: Option<&str>) -> Value {
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
    json!({
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
    })
}

fn send_book_list<R: BufRead, W: Write>(
    conn: &mut Connection<R, W>,
    lib: &Library,
    p: &Value,
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
            conn.send(
                Opcode::Ok,
                json!({
                    "priKey": index,
                    "uuid": book.get("uuid").cloned().unwrap_or(Value::Null),
                    "lpath": lpath,
                    "last_modified": book.get("last_modified").cloned().unwrap_or(json!("None")),
                    "extension": extension,
                }),
            )?;
        } else {
            conn.send(Opcode::Ok, lib.calibre_metadata(index).unwrap_or_default())?;
        }
    }
    Ok(())
}

fn receive_book<R: BufRead, W: Write>(
    conn: &mut Connection<R, W>,
    lib: &mut Library,
    cfg: &DeviceConfig,
    p: &Value,
) -> io::Result<()> {
    let length = p["length"]
        .as_u64()
        .ok_or_else(|| protocol_error("SEND_BOOK without length"))?;
    let lpath = str_field(p, "lpath");
    let wants_ok = p["wantsSendOkToSendbook"].as_bool().unwrap_or(false);
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
        let mut out = Vec::new();
        let end = {
            let mut conn = Connection::new(Cursor::new(script.0), &mut out);
            run(&mut conn, lib, cfg, &mut |info| {
                if info.library_uuid == "lib-1" {
                    Admission::Accept {
                        password: password.map(str::to_owned),
                    }
                } else {
                    Admission::Reject
                }
            })
            .unwrap()
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
