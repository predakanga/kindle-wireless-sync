// SPDX-License-Identifier: GPL-3.0-or-later
//! The content catalog, for series (ADR 0016).
//!
//! Writes go through the catalog's own ChangeRequest endpoint (`POST
//! 127.0.0.1:9101/change`, authorised by `/tmp/session_token`), whose
//! triggers keep `cc.db` and the library's asset store in step. Reads open
//! `cc.db` read-only with our own SQLite (ADR 0025). We never write either
//! database (ADR 0005). See `docs/research/kindle-library-integration.md`.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, ToSql};
use serde_json::{json, Value};

pub const ENDPOINT: &str = "127.0.0.1:9101";
pub const TOKEN_FILE: &str = "/tmp/session_token";
/// LibrarianSync found larger requests can run the catalog out of memory.
const BATCH: usize = 100;
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
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

    /// Whether the catalog can take changes now. The token appears once the
    /// framework is up, which can be after our service starts at boot.
    pub fn ready(&self) -> bool {
        self.db.is_file() && self.token_file.is_file()
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

    /// [`Catalog::change`], trying again while the catalog's database is
    /// locked: the scanner holds it while it adds or removes a book. Up to
    /// `attempts` tries, `wait` apart.
    pub fn change_retrying(
        &self,
        commands: &[Value],
        attempts: u32,
        wait: Duration,
    ) -> io::Result<()> {
        let mut tries = 1;
        loop {
            match self.change(commands) {
                Err(e) if tries < attempts && e.to_string().contains("database is locked") => {
                    std::thread::sleep(wait);
                    tries += 1;
                }
                result => return result,
            }
        }
    }

    /// [`query`] on the catalog.
    pub fn query(
        &self,
        sql: &str,
        params: &[&dyn ToSql],
    ) -> io::Result<Vec<HashMap<String, Value>>> {
        query(&self.db, sql, params)
    }
}

/// Runs a read-only query on `db` and returns its rows, each a map of
/// column name to value. Fails at once if the database is busy (`database
/// is locked`), with no busy timeout; callers retry (ADR 0025).
pub fn query(
    db: &Path,
    sql: &str,
    params: &[&dyn ToSql],
) -> io::Result<Vec<HashMap<String, Value>>> {
    let run = || -> rusqlite::Result<Vec<HashMap<String, Value>>> {
        let conn = Connection::open_with_flags(
            db,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(Duration::ZERO)?;
        let mut statement = conn.prepare(sql)?;
        let names: Vec<String> = statement
            .column_names()
            .into_iter()
            .map(String::from)
            .collect();
        let rows = statement.query_map(params, |row| {
            names
                .iter()
                .enumerate()
                .map(|(i, name)| Ok((name.clone(), json_value(row.get_ref(i)?))))
                .collect()
        })?;
        rows.collect()
    };
    run().map_err(|e| io::Error::other(format!("{}: {e}", db.display())))
}

/// A column value as JSON, as `sqlite3 -json` gave it.
fn json_value(v: ValueRef) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => i.into(),
        ValueRef::Real(f) => json!(f),
        ValueRef::Text(t) | ValueRef::Blob(t) => String::from_utf8_lossy(t).into(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::path::Path;

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
    fn a_locked_catalog_is_tried_again() {
        let tmp = tempfile::tempdir().unwrap();
        let locked =
            r#"{ "Error": "Failed to execute handler: {\"sqlite3_msg\":\"database is locked\"}" }"#;
        let (addr, t) = fake(vec![
            ("503", locked),
            ("503", locked),
            ("200", r#"{"ok":true}"#),
        ]);
        let c = catalog(addr, tmp.path());
        c.change_retrying(&[json!({})], 5, Duration::ZERO).unwrap();
        assert_eq!(t.join().unwrap().len(), 3);
        // Other errors, and too many locks, fail.
        let (addr, t) = fake(vec![("503", locked), ("503", locked)]);
        assert!(catalog(addr, tmp.path())
            .change_retrying(&[json!({})], 2, Duration::ZERO)
            .is_err());
        t.join().unwrap();
        let (addr, t) = fake(vec![("401", r#"{ "Error": "Unauthorized request." }"#)]);
        assert!(catalog(addr, tmp.path())
            .change_retrying(&[json!({})], 5, Duration::ZERO)
            .is_err());
        assert_eq!(t.join().unwrap().len(), 1);
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
    fn queries_return_rows_as_json() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("cc.db");
        Connection::open(&db)
            .unwrap()
            .execute_batch(
                "CREATE TABLE t (a, b, c, d); \
                 INSERT INTO t VALUES ('x|y', 2, NULL, 1.5), ('it''s', 3, 'z', 0);",
            )
            .unwrap();
        let rows = query(&db, "SELECT * FROM t WHERE a = ?1", &[&"x|y"]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["a"], "x|y");
        assert_eq!(rows[0]["b"], 2);
        assert_eq!(rows[0]["c"], Value::Null);
        assert_eq!(rows[0]["d"], 1.5);
        assert_eq!(
            query(&db, "SELECT a FROM t WHERE a = ?1", &[&"it's"])
                .unwrap()
                .len(),
            1
        );
        assert!(query(&db, "SELECT * FROM t WHERE b > 9", &[])
            .unwrap()
            .is_empty());
        // Read-only: writes fail, and nothing changes.
        assert!(query(&db, "DELETE FROM t", &[]).is_err());
        assert_eq!(query(&db, "SELECT * FROM t", &[]).unwrap().len(), 2);
        assert!(query(&tmp.path().join("missing.db"), "SELECT 1", &[]).is_err());
    }

    #[test]
    fn a_busy_database_fails_at_once() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("cc.db");
        let holder = Connection::open(&db).unwrap();
        holder
            .execute_batch("CREATE TABLE t (a); BEGIN EXCLUSIVE; INSERT INTO t VALUES (1);")
            .unwrap();
        let start = std::time::Instant::now();
        let err = query(&db, "SELECT * FROM t", &[]).unwrap_err();
        assert!(err.to_string().contains("database is locked"), "{err}");
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
