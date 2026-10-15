//! The long-running client (ADR 0004, ADR 0008).
//!
//! Waits for Wi-Fi, connects only to allowed Calibre libraries, runs
//! sessions, and keeps a status for the UI. After Calibre ejects us it stays
//! disconnected until Wi-Fi cycles or "Connect now". New libraries are only
//! contacted by the explicit [`find_libraries`] and [`add_server`] actions.

use std::collections::HashSet;
use std::fs;
use std::io::{self, BufReader, BufWriter};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::config::{AllowedLibrary, Config};
use crate::discovery::CalibreServer;
use crate::library::Library;
use crate::protocol::{read_message, Connection, Opcode};
use crate::session::{self, Admission, CalibreInfo, DeviceConfig, SessionEnd};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// How often to re-read the config while no library is allowed, so a
/// library allowed from another process is picked up.
const NO_LIBRARY_RECHECK: Duration = Duration::from_secs(30);
pub const STATUS_FILE: &str = "status.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    WaitingForWifi,
    NoLibraries,
    Searching,
    NotFound,
    Connected,
    Ejected,
    PasswordRejected,
    Error,
    Stopped,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::WaitingForWifi => "waiting_for_wifi",
            State::NoLibraries => "no_libraries",
            State::Searching => "searching",
            State::NotFound => "not_found",
            State::Connected => "connected",
            State::Ejected => "ejected",
            State::PasswordRejected => "password_rejected",
            State::Error => "error",
            State::Stopped => "stopped",
        }
    }
}

/// What the UI shows: a state, a reason in plain words, and since when.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub state: State,
    pub detail: String,
    /// Unix seconds.
    pub since: u64,
}

pub enum Control {
    Network(bool),
    ConnectNow,
    Shutdown,
}

struct Shared {
    status: Mutex<Status>,
    /// The socket of the running session, so it can be cut when Wi-Fi drops.
    active: Mutex<Option<TcpStream>>,
    status_file: PathBuf,
}

impl Shared {
    fn set(&self, state: State, detail: String) {
        let mut status = self.status.lock().unwrap();
        if status.state == state && status.detail == detail {
            return;
        }
        log::info!("status: {} - {detail}", state.as_str());
        *status = Status {
            state,
            detail,
            since: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        let body =
            json!({"state": status.state.as_str(), "detail": status.detail, "since": status.since});
        let tmp = self.status_file.with_extension("json.tmp");
        if let Err(e) =
            fs::write(&tmp, body.to_string()).and_then(|_| fs::rename(&tmp, &self.status_file))
        {
            log::warn!("cannot write {}: {e}", self.status_file.display());
        }
    }

    fn interrupt(&self) {
        if let Some(stream) = self.active.lock().unwrap().as_ref() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

/// Controls a running daemon from other threads.
#[derive(Clone)]
pub struct Handle {
    tx: Sender<Control>,
    shared: Arc<Shared>,
}

impl Handle {
    /// Reports a Wi-Fi change. Going down also cuts the current session at
    /// once rather than waiting for a TCP timeout.
    pub fn network_changed(&self, up: bool) {
        if !up {
            self.shared.interrupt();
        }
        let _ = self.tx.send(Control::Network(up));
    }

    pub fn connect_now(&self) {
        let _ = self.tx.send(Control::ConnectNow);
    }

    pub fn shutdown(&self) {
        self.shared.interrupt();
        let _ = self.tx.send(Control::Shutdown);
    }

    pub fn status(&self) -> Status {
        self.shared.status.lock().unwrap().clone()
    }
}

pub struct Options {
    pub device: DeviceConfig,
    pub state_dir: PathBuf,
    /// Delays between retries; the last one repeats.
    pub backoff: Vec<Duration>,
    /// Silence on a session this long means the connection is dead.
    pub read_timeout: Duration,
    /// Finds Calibre servers by broadcast.
    pub discover: Box<dyn FnMut() -> Vec<CalibreServer> + Send>,
}

impl Options {
    pub fn default_backoff() -> Vec<Duration> {
        [10, 30, 120].into_iter().map(Duration::from_secs).collect()
    }
}

pub struct Daemon {
    lib: Library,
    opts: Options,
    rx: Receiver<Control>,
    shared: Arc<Shared>,
    net_up: bool,
}

enum Wake {
    Network(bool),
    ConnectNow,
    Shutdown,
    Timeout,
}

#[derive(Debug, Clone, PartialEq)]
struct Candidate {
    addr: SocketAddr,
    hostname: String,
}

/// How one pass over the candidates ended.
enum Outcome {
    NotFound,
    Busy,
    Ejected,
    PasswordRejected(String),
    Lost(String),
}

impl Daemon {
    pub fn new(lib: Library, opts: Options, net_up: bool) -> (Daemon, Handle) {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            status: Mutex::new(Status {
                state: State::Stopped,
                detail: String::new(),
                since: 0,
            }),
            active: Mutex::new(None),
            status_file: opts.state_dir.join(STATUS_FILE),
        });
        let handle = Handle {
            tx,
            shared: shared.clone(),
        };
        (
            Daemon {
                lib,
                opts,
                rx,
                shared,
                net_up,
            },
            handle,
        )
    }

    fn set(&self, state: State, detail: impl Into<String>) {
        self.shared.set(state, detail.into());
    }

    fn wait(&self, timeout: Option<Duration>) -> Wake {
        let msg = match timeout {
            Some(t) => match self.rx.recv_timeout(t) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => return Wake::Timeout,
                Err(RecvTimeoutError::Disconnected) => return Wake::Shutdown,
            },
            None => match self.rx.recv() {
                Ok(m) => m,
                Err(_) => return Wake::Shutdown,
            },
        };
        match msg {
            Control::Network(up) => Wake::Network(up),
            Control::ConnectNow => Wake::ConnectNow,
            Control::Shutdown => Wake::Shutdown,
        }
    }

    /// Runs until [`Handle::shutdown`].
    pub fn run(mut self) {
        // Reset whenever Wi-Fi cycles (ADR 0008).
        let mut ejected = false;
        let mut skip: HashSet<SocketAddr> = HashSet::new();
        let mut bad_password: HashSet<String> = HashSet::new();
        let mut attempt = 0usize;

        loop {
            if !self.net_up {
                self.set(State::WaitingForWifi, "Waiting for Wi-Fi.");
                match self.wait(None) {
                    Wake::Network(true) => {
                        self.net_up = true;
                        ejected = false;
                        skip.clear();
                        bad_password.clear();
                        attempt = 0;
                    }
                    Wake::Shutdown => break,
                    _ => {}
                }
                continue;
            }
            if ejected {
                self.set(
                    State::Ejected,
                    "Disconnected by Calibre. Will reconnect when Wi-Fi reconnects, or use Connect now.",
                );
                match self.wait(None) {
                    Wake::Network(false) => self.net_up = false,
                    Wake::ConnectNow => {
                        ejected = false;
                        attempt = 0;
                    }
                    Wake::Shutdown => break,
                    _ => {}
                }
                continue;
            }

            let config = match Config::load(&self.opts.state_dir) {
                Ok(c) => c,
                Err(e) => {
                    self.set(State::Error, format!("Cannot read settings: {e}"));
                    match self.wait(Some(NO_LIBRARY_RECHECK)) {
                        Wake::Network(false) => self.net_up = false,
                        Wake::Shutdown => break,
                        _ => {}
                    }
                    continue;
                }
            };
            if config.allowed.is_empty() {
                self.set(
                    State::NoLibraries,
                    "No Calibre library is allowed yet. Use Find libraries or Add server.",
                );
                match self.wait(Some(NO_LIBRARY_RECHECK)) {
                    Wake::Network(false) => self.net_up = false,
                    Wake::Shutdown => break,
                    _ => {}
                }
                continue;
            }
            let allowed: Vec<&AllowedLibrary> = config
                .allowed
                .iter()
                .filter(|a| !bad_password.contains(&a.uuid))
                .collect();
            if allowed.is_empty() {
                // Every allowed library rejected our password; wait for the
                // user (Connect now after fixing it) or a Wi-Fi cycle.
                match self.wait(None) {
                    Wake::Network(false) => self.net_up = false,
                    Wake::ConnectNow => {
                        bad_password.clear();
                        attempt = 0;
                    }
                    Wake::Shutdown => break,
                    _ => {}
                }
                continue;
            }

            let names: Vec<&str> = allowed.iter().map(|a| display_name(a)).collect();
            self.set(
                State::Searching,
                format!("Looking for {}.", names.join(", ")),
            );
            let found = (self.opts.discover)();
            let candidates = candidates(&allowed, &found, &skip);

            let mut outcome = Outcome::NotFound;
            for cand in candidates {
                match self.try_session(&cand, &config, &bad_password) {
                    Err(e) => log::debug!("cannot reach {}: {e}", cand.addr),
                    Ok((end, info)) => {
                        let name = info
                            .as_ref()
                            .map(|i| i.library_name.clone())
                            .unwrap_or_default();
                        match end {
                            Ok(SessionEnd::Ejected) => {
                                outcome = Outcome::Ejected;
                                break;
                            }
                            Ok(SessionEnd::LibraryRejected(info)) => {
                                log::warn!(
                                    "{} now serves library {:?}, which is not allowed",
                                    cand.addr,
                                    info.library_name
                                );
                                self.record_seen(&info, &cand);
                                skip.insert(cand.addr);
                            }
                            Ok(SessionEnd::PasswordRejected) => {
                                if let Some(i) = &info {
                                    bad_password.insert(i.library_uuid.clone());
                                }
                                outcome = Outcome::PasswordRejected(name);
                                break;
                            }
                            Ok(SessionEnd::Busy) => outcome = Outcome::Busy,
                            Ok(SessionEnd::Closed | SessionEnd::ProtocolMismatch) | Err(_) => {
                                if let Err(e) = &end {
                                    log::warn!("session with {} failed: {e}", cand.addr);
                                }
                                // We were connected, so start the backoff over.
                                attempt = 0;
                                outcome = Outcome::Lost(name);
                                break;
                            }
                        }
                    }
                }
            }

            // A session cut by Wi-Fi going down: report that, not a lost
            // connection. Apply any network changes that arrived meanwhile.
            let mut went_down = false;
            let mut shutdown = false;
            while let Ok(msg) = self.rx.try_recv() {
                match msg {
                    Control::Network(false) => {
                        went_down = true;
                        self.net_up = false;
                    }
                    Control::Network(true) => self.net_up = true,
                    Control::ConnectNow => attempt = 0,
                    Control::Shutdown => shutdown = true,
                }
            }
            if shutdown {
                break;
            }
            if went_down {
                if self.net_up {
                    // Down and back up already: a Wi-Fi cycle.
                    ejected = false;
                    skip.clear();
                    bad_password.clear();
                    attempt = 0;
                }
                continue;
            }

            let delay = self.opts.backoff[attempt.min(self.opts.backoff.len() - 1)];
            let secs = delay.as_secs();
            match outcome {
                Outcome::Ejected => {
                    ejected = true;
                    attempt = 0;
                    continue;
                }
                Outcome::PasswordRejected(name) => {
                    self.set(
                        State::PasswordRejected,
                        format!("Calibre library {name} rejected the password. Fix it, then use Connect now."),
                    );
                    continue;
                }
                Outcome::NotFound => self.set(
                    State::NotFound,
                    format!("No allowed Calibre library found. Retrying in {secs}s."),
                ),
                Outcome::Busy => self.set(
                    State::NotFound,
                    format!("Calibre is busy with another device. Retrying in {secs}s."),
                ),
                Outcome::Lost(name) => self.set(
                    State::NotFound,
                    format!("Lost the connection to {name}. Retrying in {secs}s."),
                ),
            }
            attempt += 1;
            match self.wait(Some(delay)) {
                Wake::Network(false) => self.net_up = false,
                Wake::ConnectNow => attempt = 0,
                Wake::Shutdown => break,
                Wake::Network(true) | Wake::Timeout => {}
            }
        }
        self.set(State::Stopped, "Stopped.");
    }

    /// Connects and runs one session. `Err` means we never got a session
    /// going (unreachable); otherwise the session result and the library
    /// that answered.
    #[allow(clippy::type_complexity)]
    fn try_session(
        &mut self,
        cand: &Candidate,
        config: &Config,
        bad_password: &HashSet<String>,
    ) -> io::Result<(io::Result<SessionEnd>, Option<CalibreInfo>)> {
        let stream = TcpStream::connect_timeout(&cand.addr, CONNECT_TIMEOUT)?;
        stream.set_read_timeout(Some(self.opts.read_timeout))?;
        stream.set_nodelay(true)?;
        *self.shared.active.lock().unwrap() = Some(stream.try_clone()?);
        let mut conn = Connection::new(BufReader::new(stream.try_clone()?), BufWriter::new(stream));

        let shared = self.shared.clone();
        let mut answered: Option<CalibreInfo> = None;
        let mut admitted = false;
        let end = session::run(&mut conn, &mut self.lib, &self.opts.device, &mut |info| {
            answered = Some(info.clone());
            match config.allowed(&info.library_uuid) {
                Some(a) if !bad_password.contains(&a.uuid) => {
                    admitted = true;
                    shared.set(
                        State::Connected,
                        format!("Connected to {}.", info.library_name),
                    );
                    Admission::Accept {
                        password: a.password.clone(),
                    }
                }
                _ => Admission::Reject,
            }
        });
        *self.shared.active.lock().unwrap() = None;

        if admitted {
            if let Some(info) = &answered {
                self.record_connected(info, cand);
            }
        }
        Ok((end, answered))
    }

    fn record_connected(&self, info: &CalibreInfo, cand: &Candidate) {
        let result = Config::load(&self.opts.state_dir).and_then(|mut c| {
            let previous = c.allowed(&info.library_uuid).cloned().unwrap_or_default();
            c.upsert_allowed(AllowedLibrary {
                uuid: info.library_uuid.clone(),
                name: info.library_name.clone(),
                host: Some(cand.addr.ip().to_string()),
                port: Some(cand.addr.port()),
                hostname: (!cand.hostname.is_empty())
                    .then(|| cand.hostname.clone())
                    .or(previous.hostname),
                password: None,
            });
            c.save()
        });
        if let Err(e) = result {
            log::warn!("cannot update settings: {e}");
        }
    }

    fn record_seen(&self, info: &CalibreInfo, cand: &Candidate) {
        let result = Config::load(&self.opts.state_dir).and_then(|mut c| {
            c.record_seen(
                &info.library_uuid,
                &info.library_name,
                &cand.addr.ip().to_string(),
                cand.addr.port(),
                &cand.hostname,
            );
            c.save()
        });
        if let Err(e) = result {
            log::warn!("cannot update settings: {e}");
        }
    }
}

fn display_name(a: &AllowedLibrary) -> &str {
    if a.name.is_empty() {
        &a.uuid
    } else {
        &a.name
    }
}

/// Addresses worth connecting to in the background: discovered servers that
/// match an allowed library, then each allowed library's last address.
fn candidates(
    allowed: &[&AllowedLibrary],
    found: &[CalibreServer],
    skip: &HashSet<SocketAddr>,
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut push = |addr: SocketAddr, hostname: String| {
        if !skip.contains(&addr) && !out.iter().any(|c| c.addr == addr) {
            out.push(Candidate { addr, hostname });
        }
    };
    for s in found {
        let ip = s.addr.to_string();
        let matches = allowed.iter().any(|a| {
            a.hostname.as_deref() == Some(s.hostname.as_str())
                || a.host.as_deref() == Some(ip.as_str())
        });
        if matches {
            push(SocketAddr::new(s.addr, s.port), s.hostname.clone());
        }
    }
    for a in allowed {
        if let (Some(host), Some(port)) = (&a.host, a.port) {
            match (host.as_str(), port).to_socket_addrs() {
                Ok(mut addrs) => {
                    if let Some(addr) = addrs.next() {
                        push(addr, a.hostname.clone().unwrap_or_default());
                    }
                }
                Err(e) => log::debug!("cannot resolve {host}: {e}"),
            }
        }
    }
    out
}

/// Connects to `addr`, reads which library Calibre serves, and disconnects
/// without completing the handshake.
pub fn probe(addr: SocketAddr) -> io::Result<CalibreInfo> {
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    stream.set_read_timeout(Some(PROBE_READ_TIMEOUT))?;
    let mut reader = BufReader::new(stream);
    match read_message(&mut reader)? {
        Some(m) if m.opcode() == Some(Opcode::GetInitializationInfo) => {
            Ok(CalibreInfo::from_init_payload(&m.payload))
        }
        Some(m) if m.opcode() == Some(Opcode::CalibreBusy) => {
            Err(io::Error::other("Calibre is busy with another device"))
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a Calibre wireless device connection",
        )),
    }
}

/// "Find libraries": probes each discovered server that is not already an
/// allowed library's, and records what answers in `seen`.
pub fn find_libraries(
    state_dir: &Path,
    found: &[CalibreServer],
) -> io::Result<Vec<(CalibreServer, io::Result<CalibreInfo>)>> {
    let mut config = Config::load(state_dir)?;
    let mut results = Vec::new();
    for s in found {
        let addr = SocketAddr::new(s.addr, s.port);
        let result = probe(addr);
        if let Ok(info) = &result {
            if config.allowed(&info.library_uuid).is_none() {
                config.record_seen(
                    &info.library_uuid,
                    &info.library_name,
                    &s.addr.to_string(),
                    s.port,
                    &s.hostname,
                );
            }
        }
        results.push((s.clone(), result));
    }
    config.save()?;
    Ok(results)
}

/// "Add server": probes `host:port` now. On success the library that answers
/// is allowed with this address; on failure nothing is stored (ADR 0008).
pub fn add_server(
    state_dir: &Path,
    host: &str,
    port: u16,
    password: Option<String>,
) -> io::Result<CalibreInfo> {
    let addr = (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("cannot resolve {host}")))?;
    let info = probe(addr)?;
    let mut config = Config::load(state_dir)?;
    config.upsert_allowed(AllowedLibrary {
        uuid: info.library_uuid.clone(),
        name: info.library_name.clone(),
        host: Some(host.to_owned()),
        port: Some(port),
        hostname: None,
        password,
    });
    config.save()?;
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::write_message;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    /// A fake Calibre: on each connection it announces `uuid`, and if the
    /// device answers, ejects it.
    fn fake_calibre(uuid: &'static str) -> (SocketAddr, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                c.fetch_add(1, Ordering::SeqCst);
                let init = json!({"serverProtocolVersion": 1, "currentLibraryUUID": uuid,
                                  "currentLibraryName": "Test Library", "calibre_version": [9, 16, 0],
                                  "passwordChallenge": ""});
                if write_message(&mut stream, Opcode::GetInitializationInfo, &init).is_err() {
                    continue;
                }
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                if !matches!(read_message(&mut reader), Ok(Some(_))) {
                    continue; // device refused or probed
                }
                let _ = write_message(&mut stream, Opcode::Noop, &json!({"ejecting": true}));
                let _ = read_message(&mut reader);
            }
        });
        (addr, count)
    }

    fn start(
        state: &Path,
        allowed: Option<(&str, SocketAddr)>,
    ) -> (Handle, std::thread::JoinHandle<()>) {
        let mut config = Config::load(state).unwrap();
        if let Some((uuid, addr)) = allowed {
            config.upsert_allowed(AllowedLibrary {
                uuid: uuid.into(),
                name: "Test Library".into(),
                host: Some(addr.ip().to_string()),
                port: Some(addr.port()),
                ..Default::default()
            });
        }
        config.save().unwrap();
        let lib = Library::open(&state.join("books"), state).unwrap();
        let opts = Options {
            device: DeviceConfig::default(),
            state_dir: state.to_path_buf(),
            backoff: vec![Duration::from_millis(50)],
            read_timeout: Duration::from_secs(5),
            discover: Box::new(Vec::new),
        };
        let (daemon, handle) = Daemon::new(lib, opts, true);
        (handle, std::thread::spawn(move || daemon.run()))
    }

    fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !f() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn after_eject_waits_for_wifi_cycle_or_connect_now() {
        let tmp = tempfile::tempdir().unwrap();
        let (addr, count) = fake_calibre("lib-1");
        let (h, thread) = start(tmp.path(), Some(("lib-1", addr)));

        wait_until("first eject", || h.status().state == State::Ejected);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(count.load(Ordering::SeqCst), 1, "no reconnect after eject");

        h.connect_now();
        wait_until("reconnect on Connect now", || {
            count.load(Ordering::SeqCst) == 2
        });
        wait_until("second eject", || h.status().state == State::Ejected);

        h.network_changed(false);
        wait_until("waiting for wifi", || {
            h.status().state == State::WaitingForWifi
        });
        h.network_changed(true);
        wait_until("reconnect after wifi cycle", || {
            count.load(Ordering::SeqCst) == 3
        });

        h.shutdown();
        thread.join().unwrap();
        assert_eq!(h.status().state, State::Stopped);
        let status: serde_json::Value =
            serde_json::from_slice(&fs::read(tmp.path().join(STATUS_FILE)).unwrap()).unwrap();
        assert_eq!(status["state"], "stopped");
    }

    #[test]
    fn other_library_at_allowed_address_is_recorded_and_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let (addr, count) = fake_calibre("someone-else");
        let (h, thread) = start(tmp.path(), Some(("lib-1", addr)));

        wait_until("not found", || h.status().state == State::NotFound);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "address skipped until wifi cycles"
        );
        let config = Config::load(tmp.path()).unwrap();
        assert_eq!(config.seen.len(), 1);
        assert_eq!(config.seen[0].uuid, "someone-else");

        h.network_changed(false);
        h.network_changed(true);
        wait_until("retry after wifi cycle", || {
            count.load(Ordering::SeqCst) == 2
        });
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn empty_allowlist_contacts_nobody() {
        let tmp = tempfile::tempdir().unwrap();
        let (h, thread) = start(tmp.path(), None);
        wait_until("no libraries", || h.status().state == State::NoLibraries);
        assert!(h.status().detail.contains("Find libraries"));
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn add_server_allows_only_on_success() {
        let tmp = tempfile::tempdir().unwrap();
        let (addr, _count) = fake_calibre("lib-9");
        let info = add_server(tmp.path(), "127.0.0.1", addr.port(), Some("pw".into())).unwrap();
        assert_eq!(info.library_uuid, "lib-9");
        let config = Config::load(tmp.path()).unwrap();
        let a = config.allowed("lib-9").unwrap();
        assert_eq!(a.port, Some(addr.port()));
        assert_eq!(a.password.as_deref(), Some("pw"));

        // A port with nothing listening: error, and nothing stored.
        let dead = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        assert!(add_server(tmp.path(), "127.0.0.1", dead.port(), None).is_err());
        assert_eq!(Config::load(tmp.path()).unwrap().allowed.len(), 1);
    }

    #[test]
    fn find_libraries_records_unknown_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let (addr, _count) = fake_calibre("lib-new");
        let server = CalibreServer {
            addr: addr.ip(),
            hostname: "pc".into(),
            port: addr.port(),
            content_server_port: None,
        };
        let results = find_libraries(tmp.path(), &[server]).unwrap();
        assert_eq!(results[0].1.as_ref().unwrap().library_uuid, "lib-new");
        let config = Config::load(tmp.path()).unwrap();
        assert_eq!(config.seen[0].hostname, "pc");
        assert!(config.allowed.is_empty());
    }

    #[test]
    fn candidates_prefer_discovered_matches_then_direct_addresses() {
        let a = AllowedLibrary {
            uuid: "u".into(),
            host: Some("10.0.0.2".into()),
            port: Some(9090),
            hostname: Some("pc".into()),
            ..Default::default()
        };
        let found = vec![
            CalibreServer {
                addr: "10.0.0.7".parse().unwrap(),
                hostname: "pc".into(),
                port: 4242,
                content_server_port: None,
            },
            CalibreServer {
                addr: "10.0.0.8".parse().unwrap(),
                hostname: "other".into(),
                port: 9090,
                content_server_port: None,
            },
        ];
        let got = candidates(&[&a], &found, &HashSet::new());
        let addrs: Vec<String> = got.iter().map(|c| c.addr.to_string()).collect();
        assert_eq!(addrs, vec!["10.0.0.7:4242", "10.0.0.2:9090"]);
        let skip: HashSet<SocketAddr> = ["10.0.0.7:4242".parse().unwrap()].into();
        assert_eq!(candidates(&[&a], &found, &skip).len(), 1);
    }
}
