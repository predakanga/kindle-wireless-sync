// SPDX-License-Identifier: GPL-3.0-or-later
//! Localhost HTTP API for the on-device UI (ADR 0011).
//!
//! A deliberately small HTTP/1.1 server: one request per connection, each on
//! its own thread so status polls stay responsive during the slow `/find`
//! and `/link`. Requests whose `Origin` is a web page are refused, so
//! pages in the Kindle's browser cannot drive the daemon.

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::Link;
use crate::config::{self, Config};
use crate::daemon::{self, FindResult, FoundLibrary, Handle, LinkError, State};
use crate::discovery::CalibreServer;
use crate::session::CalibreInfo;

pub const DEFAULT_PORT: u16 = 8325;
const MAX_HEADER: usize = 16 * 1024;
const MAX_BODY: usize = 16 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Files next to the state directory (ADR 0010).
const AUTOSTART: &str = "autostart";
const AUTOSTART_DISABLED: &str = "autostart_disabled";
const BOOT_ATTEMPTS: &str = "boot_attempts";

pub struct Api {
    pub handle: Handle,
    pub state_dir: PathBuf,
    /// Where the autostart sentinel and guard files live.
    pub home_dir: PathBuf,
    pub discover: Box<dyn Fn() -> Vec<CalibreServer> + Send + Sync>,
}

/// Serves requests until the listener fails.
pub fn serve(listener: TcpListener, api: Api) {
    let api = Arc::new(api);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let api = api.clone();
        std::thread::spawn(move || {
            if let Err(e) = handle_connection(&api, stream) {
                log::debug!("api connection error: {e}");
            }
        });
    }
}

struct Request {
    method: String,
    path: String,
    origin: Option<String>,
    body: Vec<u8>,
}

enum HttpError {
    BadRequest(&'static str),
    TooLarge,
}

fn read_request(stream: &TcpStream) -> io::Result<Result<Request, HttpError>> {
    let mut reader = BufReader::new(stream.take((MAX_HEADER + MAX_BODY) as u64));
    let mut line = String::new();
    let mut header_bytes = reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Ok(Err(HttpError::BadRequest("bad request line")));
    };
    let (method, path) = (
        method.to_owned(),
        target.split('?').next().unwrap_or("").to_owned(),
    );

    let mut origin = None;
    let mut length = 0usize;
    loop {
        let mut h = String::new();
        let n = reader.read_line(&mut h)?;
        header_bytes += n;
        if header_bytes > MAX_HEADER {
            return Ok(Err(HttpError::TooLarge));
        }
        let h = h.trim_end();
        if n == 0 || h.is_empty() {
            break;
        }
        let Some((name, value)) = h.split_once(':') else {
            return Ok(Err(HttpError::BadRequest("bad header")));
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "origin" => origin = Some(value.to_owned()),
            "content-length" => match value.parse() {
                Ok(n) => length = n,
                Err(_) => return Ok(Err(HttpError::BadRequest("bad content-length"))),
            },
            _ => {}
        }
    }
    if length > MAX_BODY {
        return Ok(Err(HttpError::TooLarge));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Ok(Request {
        method,
        path,
        origin,
        body,
    }))
}

fn write_response(mut stream: &TcpStream, status: u16, body: &Value) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let text = if status == 204 {
        String::new()
    } else {
        body.to_string()
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: application/json; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST\r\n\
         Access-Control-Allow-Headers: Content-Type\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n{text}",
        text.len()
    )?;
    stream.flush()
}

/// Web pages send their own origin; the WAF app, loaded from `file://`,
/// sends `null`, `file://` or nothing (ADR 0011).
fn is_web_origin(origin: &str) -> bool {
    let o = origin.trim().to_ascii_lowercase();
    o.starts_with("http://") || o.starts_with("https://")
}

fn handle_connection(api: &Api, stream: TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    let result = respond(api, &stream);
    // Lingering close: closing with unread input (e.g. an oversized body)
    // makes TCP send a reset that can destroy our reply before the client
    // reads it.
    let _ = stream.shutdown(Shutdown::Write);
    let _ = io::copy(
        &mut (&stream).take((MAX_HEADER + MAX_BODY) as u64),
        &mut io::sink(),
    );
    result
}

fn respond(api: &Api, stream: &TcpStream) -> io::Result<()> {
    let req = match read_request(stream)? {
        Ok(r) => r,
        Err(HttpError::BadRequest(why)) => {
            return write_response(stream, 400, &json!({"ok": false, "error": why}))
        }
        Err(HttpError::TooLarge) => {
            return write_response(stream, 413, &json!({"ok": false, "error": "too large"}))
        }
    };
    log::debug!("api: {} {} origin={:?}", req.method, req.path, req.origin);
    if let Some(origin) = req.origin.as_deref().filter(|o| is_web_origin(o)) {
        log::warn!("api: refused request from origin {origin}");
        return write_response(stream, 403, &json!({"ok": false, "error": "forbidden"}));
    }
    if req.method == "OPTIONS" {
        return write_response(stream, 204, &Value::Null);
    }
    let (status, body) = route(api, &req);
    write_response(stream, status, &body)
}

fn ok() -> (u16, Value) {
    (200, json!({"ok": true}))
}

fn fail(error: impl Into<String>) -> (u16, Value) {
    (200, json!({"ok": false, "error": error.into()}))
}

fn bad_request(error: &str) -> (u16, Value) {
    (400, json!({"ok": false, "error": error}))
}

fn route(api: &Api, req: &Request) -> (u16, Value) {
    let body: Value = if req.body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(&req.body) {
            Ok(v) => v,
            Err(_) => return bad_request("body is not JSON"),
        }
    };
    let uuid = || body.get("uuid").and_then(Value::as_str).map(str::to_owned);
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/status") => (200, status_json(api)),
        ("POST", "/connect-now") => {
            api.handle.connect_now();
            ok()
        }
        // The command line changed the link (ADR 0018): end the session now.
        ("POST", "/reconnect") => {
            api.handle.reconnect();
            ok()
        }
        ("POST", "/pause") => {
            api.handle.pause();
            ok()
        }
        ("POST", "/resume") => {
            api.handle.resume();
            ok()
        }
        ("POST", "/autostart") => match body.get("enabled").and_then(Value::as_bool) {
            Some(enabled) => match set_autostart(&api.home_dir, enabled) {
                Ok(()) => ok(),
                Err(e) => fail(format!("Cannot change autostart: {e}")),
            },
            None => bad_request("expected {\"enabled\": true|false}"),
        },
        ("POST", "/find") => find(api),
        ("POST", "/link") => link(api, &body),
        ("POST", "/unlink") => match uuid() {
            Some(u) => {
                let unlinked =
                    config::update(&api.state_dir, |c| c.linked(&u).is_some() && c.unlink());
                match unlinked {
                    Ok(true) => {
                        api.handle.reconnect();
                        ok()
                    }
                    Ok(false) => fail("That library isn't linked."),
                    Err(e) => fail(format!("Cannot save settings: {e}")),
                }
            }
            None => bad_request("expected {\"uuid\": ...}"),
        },
        ("POST", "/password") => {
            let password = match body.get("password") {
                Some(Value::String(p)) if !p.is_empty() => Some(p.clone()),
                Some(Value::String(_)) | Some(Value::Null) | None => None,
                Some(_) => return bad_request("password must be a string or null"),
            };
            match uuid() {
                Some(u) => match config::update(&api.state_dir, |c| c.set_password(&u, password)) {
                    Ok(true) => {
                        api.handle.reconnect();
                        ok()
                    }
                    Ok(false) => fail("That library isn't linked."),
                    Err(e) => fail(format!("Cannot save settings: {e}")),
                },
                None => bad_request("expected {\"uuid\": ..., \"password\": ...}"),
            }
        }
        (
            _,
            "/status" | "/connect-now" | "/reconnect" | "/pause" | "/resume" | "/autostart"
            | "/find" | "/link" | "/unlink" | "/password",
        ) => (405, json!({"ok": false, "error": "method not allowed"})),
        _ => (404, json!({"ok": false, "error": "not found"})),
    }
}

fn status_json(api: &Api) -> Value {
    let status = api.handle.status();
    let mut out = json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "state": status.state.as_str(),
        "detail": status.detail,
        "since": status.since,
        "paused": status.state == State::Paused,
        "autostart": api.home_dir.join(AUTOSTART).exists(),
        "autostart_disabled_by_guard": api.home_dir.join(AUTOSTART_DISABLED).exists(),
    });
    let linked = match Config::load(&api.state_dir) {
        Ok(c) => c.library,
        Err(e) => {
            out["config_error"] = json!(e.to_string());
            None
        }
    };
    out["library"] = linked.as_ref().map_or(Value::Null, |a| {
        json!({"uuid": a.uuid, "name": a.name, "host": a.host, "port": a.port,
               "hostname": a.hostname, "has_password": a.password.is_some(),
               "writable": a.writable})
    });
    out["found"] = api
        .handle
        .found()
        .iter()
        .filter(|f| linked.as_ref().is_none_or(|a| a.uuid != f.uuid))
        .map(|f| {
            json!({"uuid": f.uuid, "name": f.name, "host": f.host, "port": f.port,
                   "hostname": f.hostname})
        })
        .collect();
    out
}

/// Asks the running daemon, through its API, to end its session and connect
/// again, so a link changed from the command line takes effect at once.
/// Returns false if no daemon is listening.
pub fn request_reconnect(port: u16) -> io::Result<bool> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = match TcpStream::connect_timeout(&addr, READ_TIMEOUT) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => return Ok(false),
        Err(e) => return Err(e),
    };
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.write_all(
        b"POST /reconnect HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    let status = reply.lines().next().unwrap_or_default();
    if status.starts_with("HTTP/1.1 200") {
        Ok(true)
    } else {
        Err(io::Error::other(format!("unexpected reply: {status}")))
    }
}

fn set_autostart(home: &Path, enabled: bool) -> io::Result<()> {
    let ignore_missing = |r: io::Result<()>| match r {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    };
    if enabled {
        fs::write(home.join(AUTOSTART), b"")?;
        ignore_missing(fs::remove_file(home.join(AUTOSTART_DISABLED)))?;
        ignore_missing(fs::remove_file(home.join(BOOT_ATTEMPTS)))
    } else {
        ignore_missing(fs::remove_file(home.join(AUTOSTART)))
    }
}

fn find(api: &Api) -> (u16, Value) {
    let found = (api.discover)();
    let config = match Config::load(&api.state_dir) {
        Ok(c) => c,
        Err(e) => return fail(format!("Cannot read settings: {e}")),
    };
    let servers: Vec<Value> = daemon::find_libraries(&config, &found)
        .iter()
        .map(|(s, r)| {
            let mut v = json!({"hostname": s.hostname, "host": s.addr.to_string(), "port": s.port});
            match r {
                FindResult::Answered(info) if config.linked(&info.library_uuid).is_some() => {
                    v["status"] = json!("linked")
                }
                FindResult::Linked => v["status"] = json!("linked"),
                FindResult::Answered(info) => {
                    api.handle.record_found(FoundLibrary::new(
                        info,
                        s.addr.to_string(),
                        s.port,
                        s.hostname.clone(),
                    ));
                    v["status"] = json!("found");
                    v["library"] = json!({"uuid": info.library_uuid, "name": info.library_name});
                }
                FindResult::Failed(e) => {
                    v["status"] = json!("error");
                    v["error"] = json!(e.to_string());
                }
            }
            v
        })
        .collect();
    (200, json!({"ok": true, "servers": servers}))
}

/// `/link`: a found library by `uuid`, or a library by `host` and `port`
/// ("Add server", optionally checking `uuid`). Replacing a different linked
/// library needs `"replace": true`; without it the reply asks for that
/// (ADR 0018).
fn link(api: &Api, body: &Value) -> (u16, Value) {
    let uuid = body.get("uuid").and_then(Value::as_str);
    let replace = body
        .get("replace")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let result = match body.get("host") {
        None => {
            let Some(uuid) = uuid else {
                return bad_request("expected {\"uuid\": ...} or {\"host\": ..., \"port\": ...}");
            };
            let Some(found) = api.handle.found().into_iter().find(|f| f.uuid == uuid) else {
                return fail("That library isn't in the found list. Use Find libraries again.");
            };
            match daemon::link_found(&api.state_dir, &found, replace) {
                Ok(Link::Refused(linked)) => Err(LinkError::NeedsReplace {
                    linked: Box::new(linked),
                    answered: CalibreInfo {
                        library_uuid: found.uuid.clone(),
                        library_name: found.name.clone(),
                        ..Default::default()
                    },
                }),
                Ok(link) => Ok((found.uuid, found.name, link)),
                Err(e) => Err(LinkError::Settings(e)),
            }
        }
        Some(host) => {
            let host = host.as_str().unwrap_or("").trim();
            let port = body.get("port").and_then(Value::as_u64);
            let password = body
                .get("password")
                .and_then(Value::as_str)
                .filter(|p| !p.is_empty())
                .map(str::to_owned);
            if host.is_empty() || host.len() > 253 || host.contains(char::is_whitespace) {
                return fail("Enter the computer's name or address.");
            }
            let Some(port) = port.filter(|p| (1..=65535).contains(p)) else {
                return fail("Enter a port between 1 and 65535.");
            };
            daemon::link_address(&api.state_dir, host, port as u16, password, uuid, replace)
                .map(|(info, link)| (info.library_uuid, info.library_name, link))
                .map_err(|e| match e {
                    LinkError::Unreachable(e) => LinkError::Unreachable(io::Error::other(format!(
                        "Could not reach Calibre at {host}:{port}: {e}"
                    ))),
                    other => other,
                })
        }
    };
    match result {
        Ok((uuid, name, link)) => {
            api.handle.forget_found(&uuid);
            let replaced = matches!(link, Link::Replaced(_));
            if replaced {
                // Stop talking to the old library at once.
                api.handle.reconnect();
            } else {
                api.handle.connect_now();
            }
            (
                200,
                json!({"ok": true, "library": {"uuid": uuid, "name": name}, "replaced": replaced}),
            )
        }
        Err(LinkError::NeedsReplace { linked, answered }) => (
            200,
            json!({
                "ok": false,
                "needs_replace": true,
                "error": format!(
                    "This Kindle is linked to {}. Linking {} replaces it.",
                    linked.display_name(),
                    display_info(&answered)
                ),
                "linked": {"uuid": linked.uuid, "name": linked.name},
                "library": {"uuid": answered.library_uuid, "name": answered.library_name},
            }),
        ),
        Err(LinkError::OtherLibrary(info)) => fail(format!(
            "A different library answered: {}.",
            display_info(&info)
        )),
        Err(LinkError::Unreachable(e)) => fail(e.to_string()),
        Err(LinkError::Settings(e)) => fail(format!("Cannot save settings: {e}")),
    }
}

fn display_info(info: &CalibreInfo) -> &str {
    if info.library_name.is_empty() {
        &info.library_uuid
    } else {
        &info.library_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LinkedLibrary;
    use crate::daemon::{Daemon, Options};
    use crate::library::Library;
    use crate::session::DeviceConfig;

    struct Fixture {
        _tmp: tempfile::TempDir,
        home: PathBuf,
        state: PathBuf,
        addr: SocketAddr,
        handle: Handle,
        // Kept alive so the handle's channel stays open; never run.
        _daemon: Daemon,
    }

    fn start() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let state = home.join("state");
        fs::create_dir_all(&state).unwrap();
        let lib = Library::open(&home.join("books"), &state).unwrap();
        let opts = Options {
            device: DeviceConfig::default(),
            state_dir: state.clone(),
            backoff: Options::default_backoff(),
            read_timeout: Duration::from_secs(5),
            discover: Box::new(Vec::new),
            device_name: Box::new(|| "Kindle".into()),
            quiet: Duration::ZERO,
        };
        let (daemon, handle) = Daemon::new(lib, opts, true);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let api = Api {
            handle: handle.clone(),
            state_dir: state.clone(),
            home_dir: home.clone(),
            discover: Box::new(Vec::new),
        };
        std::thread::spawn(move || serve(listener, api));
        Fixture {
            _tmp: tmp,
            home,
            state,
            addr,
            handle,
            _daemon: daemon,
        }
    }

    fn request(f: &Fixture, raw: &str) -> (u16, Value) {
        let mut s = TcpStream::connect(f.addr).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        let code = resp[9..12].parse().unwrap();
        let body = resp.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
        (code, serde_json::from_str(body).unwrap_or(Value::Null))
    }

    fn get(f: &Fixture, path: &str, origin: Option<&str>) -> (u16, Value) {
        let origin = origin
            .map(|o| format!("Origin: {o}\r\n"))
            .unwrap_or_default();
        request(
            f,
            &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n{origin}\r\n"),
        )
    }

    fn post(f: &Fixture, path: &str, body: &str) -> (u16, Value) {
        request(
            f,
            &format!(
                "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        )
    }

    #[test]
    fn status_reports_state_and_libraries_without_passwords() {
        let f = start();
        let (code, v) = get(&f, "/status", None);
        assert_eq!(code, 200);
        assert_eq!(v["library"], Value::Null);
        assert_eq!(v["found"], json!([]));
        config::update(&f.state, |c| {
            c.link(
                LinkedLibrary {
                    uuid: "lib-1".into(),
                    name: "Books".into(),
                    password: Some("secret".into()),
                    ..Default::default()
                },
                false,
            )
        })
        .unwrap();
        f.handle.record_found(found("lib-1"));
        f.handle.record_found(found("lib-2"));
        let (code, v) = get(&f, "/status", None);
        assert_eq!(code, 200);
        assert_eq!(v["ok"], true);
        assert!(v["state"].is_string());
        assert_eq!(v["autostart"], false);
        assert_eq!(v["library"]["has_password"], true);
        assert_eq!(v["library"]["writable"], false);
        assert!(!v.to_string().contains("secret"));
        // The linked library isn't listed as found.
        assert_eq!(v["found"].as_array().unwrap().len(), 1);
        assert_eq!(v["found"][0]["uuid"], "lib-2");
    }

    #[test]
    fn web_origins_are_refused() {
        let f = start();
        assert_eq!(get(&f, "/status", Some("http://evil.example")).0, 403);
        assert_eq!(get(&f, "/status", Some("HTTPS://evil.example")).0, 403);
        assert_eq!(get(&f, "/status", Some("null")).0, 200);
        assert_eq!(get(&f, "/status", Some("file://")).0, 200);
    }

    #[test]
    fn autostart_toggle_manages_sentinel_and_guard_marker() {
        let f = start();
        fs::write(f.home.join(AUTOSTART_DISABLED), b"").unwrap();
        fs::write(f.home.join(BOOT_ATTEMPTS), b"3").unwrap();
        assert_eq!(
            get(&f, "/status", None).1["autostart_disabled_by_guard"],
            true
        );
        assert_eq!(post(&f, "/autostart", r#"{"enabled": true}"#).1["ok"], true);
        assert!(f.home.join(AUTOSTART).exists());
        assert!(!f.home.join(AUTOSTART_DISABLED).exists());
        assert!(!f.home.join(BOOT_ATTEMPTS).exists());
        assert_eq!(
            post(&f, "/autostart", r#"{"enabled": false}"#).1["ok"],
            true
        );
        assert!(!f.home.join(AUTOSTART).exists());
        assert_eq!(post(&f, "/autostart", r#"{"on": 1}"#).0, 400);
    }

    fn found(uuid: &str) -> FoundLibrary {
        FoundLibrary {
            uuid: uuid.into(),
            name: format!("Library {uuid}"),
            host: "10.0.0.9".into(),
            port: 9090,
            hostname: "pc".into(),
            password_required: false,
        }
    }

    #[test]
    fn link_from_found_password_and_unlink() {
        let f = start();
        f.handle.record_found(found("lib-2"));
        assert_eq!(post(&f, "/link", r#"{"uuid": "nope"}"#).1["ok"], false);
        let v = post(&f, "/link", r#"{"uuid": "lib-2"}"#).1;
        assert_eq!(v["ok"], true);
        assert_eq!(v["replaced"], false);
        let a = Config::load(&f.state).unwrap().library.unwrap();
        assert_eq!(a.uuid, "lib-2");
        assert_eq!(a.host.as_deref(), Some("10.0.0.9"));
        assert!(f.handle.found().is_empty(), "no longer listed as found");

        assert_eq!(
            post(&f, "/password", r#"{"uuid": "lib-2", "password": "pw"}"#).1["ok"],
            true
        );
        assert_eq!(get(&f, "/status", None).1["library"]["has_password"], true);
        assert_eq!(
            post(&f, "/password", r#"{"uuid": "lib-2", "password": null}"#).1["ok"],
            true
        );
        assert_eq!(get(&f, "/status", None).1["library"]["has_password"], false);
        assert_eq!(
            post(&f, "/password", r#"{"uuid": "lib-1", "password": "pw"}"#).1["ok"],
            false
        );

        assert_eq!(post(&f, "/unlink", r#"{"uuid": "lib-1"}"#).1["ok"], false);
        assert_eq!(post(&f, "/unlink", r#"{"uuid": "lib-2"}"#).1["ok"], true);
        assert_eq!(Config::load(&f.state).unwrap().library, None);
        assert_eq!(post(&f, "/unlink", r#"{"uuid": "lib-2"}"#).1["ok"], false);
    }

    #[test]
    fn linking_a_different_library_needs_confirming() {
        let f = start();
        f.handle.record_found(found("lib-1"));
        f.handle.record_found(found("lib-2"));
        assert_eq!(post(&f, "/link", r#"{"uuid": "lib-1"}"#).1["ok"], true);

        let v = post(&f, "/link", r#"{"uuid": "lib-2"}"#).1;
        assert_eq!(v["ok"], false);
        assert_eq!(v["needs_replace"], true);
        assert_eq!(v["linked"]["uuid"], "lib-1");
        assert_eq!(v["library"]["name"], "Library lib-2");
        assert!(v["error"].as_str().unwrap().contains("replaces"));
        assert_eq!(
            Config::load(&f.state).unwrap().library.unwrap().uuid,
            "lib-1"
        );

        let v = post(&f, "/link", r#"{"uuid": "lib-2", "replace": true}"#).1;
        assert_eq!(v["ok"], true);
        assert_eq!(v["replaced"], true);
        assert_eq!(
            Config::load(&f.state).unwrap().library.unwrap().uuid,
            "lib-2"
        );
    }

    #[test]
    fn link_by_address_validates_and_reports_failures() {
        let f = start();
        let v = post(&f, "/link", r#"{"host": "", "port": 9090}"#).1;
        assert_eq!(v["ok"], false);
        assert_eq!(
            post(&f, "/link", r#"{"host": "pc", "port": 0}"#).1["ok"],
            false
        );
        let dead = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let v = post(
            &f,
            "/link",
            &format!(r#"{{"host": "127.0.0.1", "port": {}}}"#, dead.port()),
        )
        .1;
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("Could not reach"));
        assert_eq!(Config::load(&f.state).unwrap().library, None);
    }

    #[test]
    fn request_reconnect_reaches_a_running_daemon_only() {
        let f = start();
        assert!(request_reconnect(f.addr.port()).unwrap());
        let dead = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        assert!(!request_reconnect(dead.port()).unwrap());
    }

    #[test]
    fn bad_requests() {
        let f = start();
        assert_eq!(get(&f, "/nope", None).0, 404);
        assert_eq!(get(&f, "/link", None).0, 405);
        assert_eq!(post(&f, "/link", "not json").0, 400);
        assert_eq!(post(&f, "/link", "{}").0, 400);
        assert_eq!(post(&f, "/allow", "{}").0, 404);
        assert_eq!(request(&f, "garbage\r\n\r\n").0, 400);
        let big = "x".repeat(MAX_BODY + 1);
        assert_eq!(post(&f, "/link", &big).0, 413);
        assert_eq!(request(&f, "OPTIONS /status HTTP/1.1\r\n\r\n").0, 204);
        assert_eq!(post(&f, "/connect-now", "").1["ok"], true);
        assert_eq!(get(&f, "/reconnect", None).0, 405);
        assert_eq!(post(&f, "/find", "").1["servers"], json!([]));
    }
}
