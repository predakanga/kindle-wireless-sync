//! Localhost HTTP API for the on-device UI (ADR 0011).
//!
//! A deliberately small HTTP/1.1 server: one request per connection, each on
//! its own thread so status polls stay responsive during the slow `/find`
//! and `/add-server`. Requests whose `Origin` is a web page are refused, so
//! pages in the Kindle's browser cannot drive the daemon.

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::{self, Config};
use crate::daemon::{self, FindResult, Handle, State};
use crate::discovery::CalibreServer;

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
        ("POST", "/add-server") => add_server(api, &body),
        ("POST", "/allow") => match uuid() {
            Some(u) => match config::update(&api.state_dir, |c| c.allow_seen(&u)) {
                Ok(true) => {
                    api.handle.connect_now();
                    ok()
                }
                Ok(false) => fail("That library isn't in the found list."),
                Err(e) => fail(format!("Cannot save settings: {e}")),
            },
            None => bad_request("expected {\"uuid\": ...}"),
        },
        ("POST", "/forget") => match uuid() {
            Some(u) => match config::update(&api.state_dir, |c| c.forget(&u)) {
                Ok(true) => {
                    api.handle.reconnect();
                    ok()
                }
                Ok(false) => fail("That library isn't allowed."),
                Err(e) => fail(format!("Cannot save settings: {e}")),
            },
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
                    Ok(false) => fail("That library isn't allowed."),
                    Err(e) => fail(format!("Cannot save settings: {e}")),
                },
                None => bad_request("expected {\"uuid\": ..., \"password\": ...}"),
            }
        }
        (
            _,
            "/status" | "/connect-now" | "/pause" | "/resume" | "/autostart" | "/find"
            | "/add-server" | "/allow" | "/forget" | "/password",
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
    match Config::load(&api.state_dir) {
        Ok(c) => {
            out["allowed"] = c
                .allowed
                .iter()
                .map(|a| {
                    json!({"uuid": a.uuid, "name": a.name, "host": a.host, "port": a.port,
                           "hostname": a.hostname, "has_password": a.password.is_some()})
                })
                .collect();
            out["seen"] = c
                .seen
                .iter()
                .map(|s| {
                    json!({"uuid": s.uuid, "name": s.name, "host": s.host, "port": s.port,
                           "hostname": s.hostname, "last_seen": s.last_seen})
                })
                .collect();
        }
        Err(e) => out["config_error"] = json!(e.to_string()),
    }
    out
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
    let results = match daemon::find_libraries(&api.state_dir, &found) {
        Ok(r) => r,
        Err(e) => return fail(format!("Cannot save settings: {e}")),
    };
    let servers: Vec<Value> = results
        .iter()
        .map(|(s, r)| {
            let mut v = json!({"hostname": s.hostname, "host": s.addr.to_string(), "port": s.port});
            match r {
                FindResult::Allowed => v["status"] = json!("allowed"),
                FindResult::Answered(info) => {
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

fn add_server(api: &Api, body: &Value) -> (u16, Value) {
    let host = body
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
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
    match daemon::add_server(&api.state_dir, host, port as u16, password) {
        Ok(info) => {
            api.handle.connect_now();
            (
                200,
                json!({"ok": true, "library": {"uuid": info.library_uuid, "name": info.library_name}}),
            )
        }
        Err(e) => fail(format!("Could not reach Calibre at {host}:{port}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AllowedLibrary;
    use crate::daemon::{Daemon, Options};
    use crate::library::Library;
    use crate::session::DeviceConfig;
    use std::net::SocketAddr;

    struct Fixture {
        _tmp: tempfile::TempDir,
        home: PathBuf,
        state: PathBuf,
        addr: SocketAddr,
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
        };
        let (daemon, handle) = Daemon::new(lib, opts, true);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let api = Api {
            handle,
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
        config::update(&f.state, |c| {
            c.upsert_allowed(AllowedLibrary {
                uuid: "lib-1".into(),
                name: "Books".into(),
                password: Some("secret".into()),
                ..Default::default()
            });
            c.record_seen("lib-2", "Other", "10.0.0.9", 9090, "pc");
        })
        .unwrap();
        let (code, v) = get(&f, "/status", None);
        assert_eq!(code, 200);
        assert_eq!(v["ok"], true);
        assert!(v["state"].is_string());
        assert_eq!(v["autostart"], false);
        assert_eq!(v["allowed"][0]["has_password"], true);
        assert!(!v.to_string().contains("secret"));
        assert_eq!(v["seen"][0]["uuid"], "lib-2");
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

    #[test]
    fn library_management() {
        let f = start();
        config::update(&f.state, |c| {
            c.record_seen("lib-2", "Other", "10.0.0.9", 9090, "pc")
        })
        .unwrap();
        assert_eq!(post(&f, "/allow", r#"{"uuid": "nope"}"#).1["ok"], false);
        assert_eq!(post(&f, "/allow", r#"{"uuid": "lib-2"}"#).1["ok"], true);
        assert!(Config::load(&f.state).unwrap().allowed("lib-2").is_some());

        assert_eq!(
            post(&f, "/password", r#"{"uuid": "lib-2", "password": "pw"}"#).1["ok"],
            true
        );
        assert_eq!(
            get(&f, "/status", None).1["allowed"][0]["has_password"],
            true
        );
        assert_eq!(
            post(&f, "/password", r#"{"uuid": "lib-2", "password": null}"#).1["ok"],
            true
        );
        assert_eq!(
            get(&f, "/status", None).1["allowed"][0]["has_password"],
            false
        );

        assert_eq!(post(&f, "/forget", r#"{"uuid": "lib-2"}"#).1["ok"], true);
        assert!(Config::load(&f.state).unwrap().allowed.is_empty());
        assert_eq!(post(&f, "/forget", r#"{"uuid": "lib-2"}"#).1["ok"], false);
    }

    #[test]
    fn add_server_validates_and_reports_failures() {
        let f = start();
        let v = post(&f, "/add-server", r#"{"host": "", "port": 9090}"#).1;
        assert_eq!(v["ok"], false);
        assert_eq!(
            post(&f, "/add-server", r#"{"host": "pc", "port": 0}"#).1["ok"],
            false
        );
        let dead = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let v = post(
            &f,
            "/add-server",
            &format!(r#"{{"host": "127.0.0.1", "port": {}}}"#, dead.port()),
        )
        .1;
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("Could not reach"));
        assert!(Config::load(&f.state).unwrap().allowed.is_empty());
    }

    #[test]
    fn bad_requests() {
        let f = start();
        assert_eq!(get(&f, "/nope", None).0, 404);
        assert_eq!(get(&f, "/allow", None).0, 405);
        assert_eq!(post(&f, "/allow", "not json").0, 400);
        assert_eq!(request(&f, "garbage\r\n\r\n").0, 400);
        let big = "x".repeat(MAX_BODY + 1);
        assert_eq!(post(&f, "/allow", &big).0, 413);
        assert_eq!(request(&f, "OPTIONS /status HTTP/1.1\r\n\r\n").0, 204);
        assert_eq!(post(&f, "/connect-now", "").1["ok"], true);
        assert_eq!(post(&f, "/find", "").1["servers"], json!([]));
    }
}
