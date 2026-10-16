//! The content catalog, for series (ADR 0016).
//!
//! Writes go through the catalog's own ChangeRequest endpoint (`POST
//! 127.0.0.1:9101/change`, authorised by `/tmp/session_token`), whose
//! triggers keep `cc.db` and the library's asset store in step. Reads use
//! `sqlite3 -readonly` on `cc.db`. We never write either database (ADR
//! 0005). See `docs/research/kindle-library-integration.md`.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{json, Value};

pub const ENDPOINT: &str = "127.0.0.1:9101";
pub const TOKEN_FILE: &str = "/tmp/session_token";
/// LibrarianSync found larger requests can run the catalog out of memory.
const BATCH: usize = 100;
const TIMEOUT: Duration = Duration::from_secs(30);

pub struct Catalog {
    pub db: PathBuf,
    pub endpoint: String,
    pub token_file: PathBuf,
}

impl Catalog {
    pub fn new(db: PathBuf) -> Catalog {
        Catalog {
            db,
            endpoint: ENDPOINT.into(),
            token_file: TOKEN_FILE.into(),
        }
    }

    /// Sends `commands` in order, in batches. Fails on the first batch the
    /// catalog refuses; earlier batches stay applied.
    pub fn change(&self, commands: &[Value]) -> io::Result<()> {
        if commands.is_empty() {
            return Ok(());
        }
        let token = std::fs::read_to_string(&self.token_file)?;
        for (i, batch) in commands.chunks(BATCH).enumerate() {
            let body = json!({"type": "ChangeRequest", "id": i + 1, "commands": batch});
            let reply = post(&self.endpoint, "/change", token.trim(), &body.to_string())?;
            if reply["ok"] != json!(true) {
                return Err(io::Error::other(format!(
                    "catalog refused the change: {reply}"
                )));
            }
        }
        Ok(())
    }

    /// Runs a read-only query and returns its rows (`sqlite3 -json`).
    /// Fails if the catalog is busy (`database is locked`); callers retry.
    pub fn query(&self, sql: &str) -> io::Result<Vec<HashMap<String, Value>>> {
        let out = Command::new("sqlite3")
            .arg("-readonly")
            .arg("-json")
            .arg(&self.db)
            .arg(sql)
            .output()?;
        if !out.status.success() {
            return Err(io::Error::other(format!(
                "sqlite3: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        parse_rows(&out.stdout)
    }
}

/// `sqlite3 -json` prints nothing at all for an empty result.
pub fn parse_rows(stdout: &[u8]) -> io::Result<Vec<HashMap<String, Value>>> {
    if stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(Vec::new());
    }
    let rows: Vec<HashMap<String, Value>> =
        serde_json::from_slice(stdout).map_err(io::Error::other)?;
    Ok(rows)
}

/// A minimal HTTP/1.1 POST of a JSON body; returns the JSON reply.
fn post(endpoint: &str, path: &str, token: &str, body: &str) -> io::Result<Value> {
    let addr = endpoint
        .parse()
        .map_err(|e| io::Error::other(format!("{endpoint}: {e}")))?;
    let mut stream = TcpStream::connect_timeout(&addr, TIMEOUT)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {endpoint}\r\nAuthToken: {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader.read_line(&mut status)?;
    let code = status
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                length = v.trim().parse::<usize>().ok();
            }
        }
    }
    let mut body = Vec::new();
    match length {
        Some(n) => reader.take(n as u64).read_to_end(&mut body)?,
        None => reader.read_to_end(&mut body)?,
    };
    if code != "200" {
        return Err(io::Error::other(format!(
            "catalog answered HTTP {code}: {}",
            String::from_utf8_lossy(&body).trim()
        )));
    }
    serde_json::from_slice(&body).map_err(io::Error::other)
}

/// Whether this looks like a Kindle with the catalog we need.
pub fn available(db: &Path) -> bool {
    db.is_file() && Path::new(TOKEN_FILE).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A one-shot fake endpoint: records each request, answers `reply`.
    fn fake(
        replies: Vec<(&'static str, &'static str)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let t = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for (code, body) in replies {
                let (mut s, _) = listener.accept().unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut head = String::new();
                let mut len = 0;
                loop {
                    let mut line = String::new();
                    r.read_line(&mut line).unwrap();
                    if let Some(v) = line.strip_prefix("Content-Length: ") {
                        len = v.trim().parse().unwrap();
                    }
                    head.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut b = vec![0; len];
                r.read_exact(&mut b).unwrap();
                seen.push(format!("{head}{}", String::from_utf8(b).unwrap()));
                write!(
                    s,
                    "HTTP/1.1 {code} X\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            seen
        });
        (addr, t)
    }

    fn catalog(endpoint: String, dir: &Path) -> Catalog {
        let token_file = dir.join("token");
        std::fs::write(&token_file, "secret-token\n").unwrap();
        Catalog {
            db: dir.join("cc.db"),
            endpoint,
            token_file,
        }
    }

    #[test]
    fn sends_batches_with_the_token() {
        let tmp = tempfile::tempdir().unwrap();
        let ok = r#"{"ok":true,"type":"ChangeResponse","changes":1}"#;
        let (addr, t) = fake(vec![("200", ok), ("200", ok)]);
        let commands: Vec<Value> = (0..150).map(|i| json!({"update": {"uuid": i}})).collect();
        catalog(addr, tmp.path()).change(&commands).unwrap();
        let seen = t.join().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen[0].starts_with("POST /change HTTP/1.1\r\n"));
        assert!(seen[0].contains("AuthToken: secret-token\r\n"));
        let body: Value = serde_json::from_str(seen[0].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["type"], "ChangeRequest");
        assert_eq!(body["commands"].as_array().unwrap().len(), 100);
        let body2: Value = serde_json::from_str(seen[1].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body2["commands"].as_array().unwrap().len(), 50);
        assert_eq!(body2["commands"][0]["update"]["uuid"], 100);
    }

    #[test]
    fn refusals_and_errors_fail() {
        let tmp = tempfile::tempdir().unwrap();
        let (addr, t) = fake(vec![("200", r#"{"ok":false}"#)]);
        assert!(catalog(addr, tmp.path()).change(&[json!({})]).is_err());
        t.join().unwrap();
        let (addr, t) = fake(vec![("401", r#"{ "Error": "Unauthorized request." }"#)]);
        let err = catalog(addr, tmp.path()).change(&[json!({})]).unwrap_err();
        assert!(err.to_string().contains("401"), "{err}");
        t.join().unwrap();
        // Nothing to send: no connection, no token needed.
        let c = Catalog::new(tmp.path().join("cc.db"));
        c.change(&[]).unwrap();
    }

    #[test]
    fn parses_sqlite_json_output() {
        assert!(parse_rows(b"").unwrap().is_empty());
        assert!(parse_rows(b"\n").unwrap().is_empty());
        let rows = parse_rows(br#"[{"a":"x|y","b":2,"c":null}]"#).unwrap();
        assert_eq!(rows[0]["a"], "x|y");
        assert_eq!(rows[0]["b"], 2);
        assert_eq!(rows[0]["c"], Value::Null);
    }
}
