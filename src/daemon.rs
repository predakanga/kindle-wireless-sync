// SPDX-License-Identifier: GPL-3.0-or-later
//! The long-running client (ADR 0004, ADR 0008).
//!
//! Waits for Wi-Fi, connects only to the linked Calibre library (ADR 0018),
//! runs sessions, and keeps a status for the UI. After Calibre ejects us it
//! stays disconnected until Wi-Fi cycles or "Connect now", unless Calibre
//! has quit: then it waits for Calibre to answer broadcasts again
//! (ADR 0021). Other libraries
//! are only contacted by the explicit [`find_libraries`] and
//! [`link_address`] actions, and what they find is kept in memory.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufReader, BufWriter};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::check::{self, CheckStatus, Checked, Checker, Target};
use crate::config::{self, Config, Link, LinkedLibrary};
use crate::discovery::CalibreServer;
use crate::library::Library;
use crate::protocol::{read_message, Connection, Opcode};
use crate::session::{self, Admission, Admit, CalibreInfo, DeviceConfig, SessionEnd};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// How often to re-read the config while no library is linked, so a
/// library linked from another process is picked up.
const NO_LIBRARY_RECHECK: Duration = Duration::from_secs(30);
pub const STATUS_FILE: &str = "status.json";
/// How long Find libraries reuses what a server answered (ADR 0020).
pub const FIND_CACHE_FOR: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    WaitingForWifi,
    NotLinked,
    Searching,
    NotFound,
    Connected,
    Ejected,
    PasswordRejected,
    Error,
    Paused,
    Stopped,
    /// Calibre closes our connections at once: its lockout (ADR 0020).
    LockedOut,
    /// Disconnect was tapped: leaving at Calibre's next keep-alive (ADR 0022).
    Disconnecting,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::WaitingForWifi => "waiting_for_wifi",
            State::NotLinked => "not_linked",
            State::Searching => "searching",
            State::NotFound => "not_found",
            State::Connected => "connected",
            State::Ejected => "ejected",
            State::PasswordRejected => "password_rejected",
            State::Error => "error",
            State::Paused => "paused",
            State::Stopped => "stopped",
            State::LockedOut => "locked_out",
            State::Disconnecting => "disconnecting",
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
    /// When the next attempt is due (Unix seconds), while waiting to
    /// retry: the app counts down to it.
    pub retry_at: Option<u64>,
    /// What the app shows under the state while retrying, if anything
    /// beyond "not available" (`detail` adds the retry delay, for the
    /// command line).
    pub advice: Option<String>,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub enum Control {
    Pause,
    Resume,
    Network(bool),
    ConnectNow,
    Shutdown,
    /// Something changed that an idle wait should look at again (the
    /// series switch, or the catalog).
    Wake,
}

struct Shared {
    status: Mutex<Status>,
    /// The socket of the running session, so it can be cut when Wi-Fi drops.
    active: Mutex<Option<TcpStream>>,
    status_file: PathBuf,
    /// Libraries found by "Find libraries", or answering at the linked
    /// library's address, that aren't linked. Kept in memory only (ADR 0018).
    found: Mutex<Vec<FoundLibrary>>,
    /// The latest check of a library (ADR 0020).
    checker: Checker,
    /// What a check needs: where state and books live, and the device
    /// config as of the last session (for the device name).
    state_dir: PathBuf,
    books_dir: PathBuf,
    device: Mutex<DeviceConfig>,
    quiet: Duration,
    /// The address a check is using: the service's own session with it
    /// ends at a quiet moment, and the service waits until the check is
    /// done.
    hold: Mutex<Option<SocketAddr>>,
    /// Whether the service stepped aside for the hold, so releasing it
    /// should reconnect.
    held: AtomicBool,
    /// Whether Wi-Fi is up, for the UI.
    wifi: AtomicBool,
    /// The linked library's sync settings changed: end the session at a
    /// quiet moment so the next one names the new columns (ADR 0020).
    settings_changed: AtomicBool,
    /// Turns series on and off while running (ADR 0016), if managed.
    series_switch: Option<Arc<AtomicBool>>,
    /// The running session's number, counted from 1, so a late fallback
    /// cut can tell it's still the same session.
    session: AtomicU64,
    /// The user asked to end the running session: it ends at Calibre's next
    /// keep-alive, or is cut after `leave_within` (ADR 0022).
    leaving: AtomicBool,
    leave_within: Duration,
    /// What Find libraries learned from each server, and when: reused for
    /// 5 minutes so searches don't count towards Calibre's lockout
    /// (ADR 0020). Cleared when Wi-Fi changes.
    find_cache: Mutex<HashMap<SocketAddr, (Instant, CalibreInfo)>>,
}

impl Shared {
    fn set(&self, state: State, detail: String) {
        self.set_full(state, detail, None, None);
    }

    /// A state while waiting `delay` to try again, with optional advice.
    fn set_retrying(&self, state: State, detail: String, advice: Option<String>, delay: Duration) {
        self.set_full(state, detail, advice, Some(unix_now() + delay.as_secs()));
    }

    fn set_full(
        &self,
        state: State,
        detail: String,
        advice: Option<String>,
        retry_at: Option<u64>,
    ) {
        let mut status = self.status.lock().unwrap();
        if status.state == state && status.detail == detail {
            // Only the countdown moved on.
            status.retry_at = retry_at;
            status.advice = advice;
            return;
        }
        log::info!("status: {} - {detail}", state.as_str());
        *status = Status {
            state,
            detail,
            since: unix_now(),
            retry_at,
            advice,
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

    /// Ends the running session at Calibre's next keep-alive, when it's
    /// between jobs, or cuts it after `leave_within` if none comes
    /// (ADR 0022). Returns whether a session was running.
    fn leave(self: &Arc<Self>) -> bool {
        if self.active.lock().unwrap().is_none() {
            return false;
        }
        let session = self.session.load(Ordering::SeqCst);
        self.leaving.store(true, Ordering::SeqCst);
        let shared = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(shared.leave_within);
            if shared.session.load(Ordering::SeqCst) == session
                && shared.active.lock().unwrap().is_some()
            {
                log::warn!("no keep-alive from calibre; cutting the connection");
                shared.interrupt();
            }
        });
        true
    }

    fn is_held(&self, addr: SocketAddr) -> bool {
        *self.hold.lock().unwrap() == Some(addr)
    }

    /// Holds `addr` for a check, waiting (a minute at most) for the
    /// service's own session with it to end; `None` releases the hold.
    fn set_hold(&self, addr: Option<SocketAddr>, tx: &Sender<Control>) {
        *self.hold.lock().unwrap() = addr;
        match addr {
            Some(addr) => {
                let deadline = Instant::now() + Duration::from_secs(60);
                while Instant::now() < deadline {
                    let busy = self
                        .active
                        .lock()
                        .unwrap()
                        .as_ref()
                        .and_then(|s| s.peer_addr().ok())
                        == Some(addr);
                    if !busy {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
            None => {
                if self.held.swap(false, Ordering::SeqCst) {
                    let _ = tx.send(Control::ConnectNow);
                }
            }
        }
    }

    fn record_found(&self, lib: FoundLibrary) {
        let mut found = self.found.lock().unwrap();
        found.retain(|f| f.uuid != lib.uuid);
        found.push(lib);
    }

    /// Drops what Find cached for an address whose broadcast answers name
    /// other ports now (ADR 0021).
    fn note_servers(&self, servers: &[CalibreServer]) {
        self.find_cache.lock().unwrap().retain(|addr, _| {
            let mut at = servers.iter().filter(|s| s.addr == addr.ip()).peekable();
            at.peek().is_none() || at.any(|s| s.port == addr.port())
        });
    }

    /// Drops what Find cached for an address, on any port: Calibre there
    /// quit (ADR 0021).
    fn forget_address(&self, ip: IpAddr) {
        self.find_cache
            .lock()
            .unwrap()
            .retain(|addr, _| addr.ip() != ip);
    }
}

/// A Calibre library that answered, but isn't linked.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundLibrary {
    pub uuid: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub hostname: String,
    /// Whether Calibre asked for a password when it was found.
    pub password_required: bool,
}

impl FoundLibrary {
    pub fn new(info: &CalibreInfo, host: String, port: u16, hostname: String) -> FoundLibrary {
        FoundLibrary {
            uuid: info.library_uuid.clone(),
            name: info.library_name.clone(),
            host,
            port,
            hostname,
            password_required: info.password_required,
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
        self.shared.wifi.store(up, Ordering::SeqCst);
        self.shared.find_cache.lock().unwrap().clear();
        if !up {
            self.shared.interrupt();
        }
        let _ = self.tx.send(Control::Network(up));
    }

    pub fn connect_now(&self) {
        let _ = self.tx.send(Control::ConnectNow);
    }

    /// Ends the current session (if any) when Calibre is between jobs and
    /// connects again, so a changed link or password takes effect
    /// (ADR 0022).
    pub fn reconnect(&self) {
        self.shared.leave();
        let _ = self.tx.send(Control::ConnectNow);
    }

    /// Disconnects, when Calibre is between jobs, and stops syncing until
    /// [`Handle::resume`]. The process and its API keep running (ADR 0011,
    /// ADR 0022).
    pub fn pause(&self) {
        if self.shared.leave() {
            self.shared.set(
                State::Disconnecting,
                "Disconnecting once Calibre has finished what it's doing.".into(),
            );
        }
        let _ = self.tx.send(Control::Pause);
    }

    pub fn resume(&self) {
        let _ = self.tx.send(Control::Resume);
    }

    pub fn shutdown(&self) {
        self.shared.interrupt();
        let _ = self.tx.send(Control::Shutdown);
    }

    pub fn status(&self) -> Status {
        self.shared.status.lock().unwrap().clone()
    }

    /// Libraries found since the daemon started, oldest first.
    pub fn found(&self) -> Vec<FoundLibrary> {
        self.shared.found.lock().unwrap().clone()
    }

    pub fn record_found(&self, lib: FoundLibrary) {
        self.shared.record_found(lib);
    }

    /// Drops a library from the found list, once it's linked.
    pub fn forget_found(&self, uuid: &str) {
        self.shared.found.lock().unwrap().retain(|f| f.uuid != uuid);
    }

    /// Starts checking a library (ADR 0020) on its own thread, replacing
    /// any earlier check. [`Handle::check_status`] shows how it goes.
    pub fn start_check(&self, target: Target, password: Option<String>) {
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        let number = shared.checker.begin(CheckStatus::checking(&target));
        std::thread::spawn(move || {
            let env = check::Env {
                state_dir: shared.state_dir.clone(),
                books_dir: shared.books_dir.clone(),
                device: shared.device.lock().unwrap().clone(),
                quiet: shared.quiet.min(check::QUIET),
                // Tests run without the quiet wait; they need no long ones.
                busy_wait: if shared.quiet.is_zero() {
                    Duration::from_millis(20)
                } else {
                    check::BUSY_WAIT
                },
            };
            let status = check::run(&env, &target, password, &|addr| shared.set_hold(addr, &tx));
            log::info!("check of {}: {}", status.name, status.state.as_str());
            shared.checker.finish(number, status);
        });
    }

    /// Wakes an idle wait, so it looks at pending work again (the catalog
    /// changed, say).
    pub fn wake(&self) {
        let _ = self.tx.send(Control::Wake);
    }

    /// Turns series on or off now, when the service manages them; the
    /// setting itself is in `config.json`.
    pub fn set_series(&self, enabled: bool) {
        if let Some(switch) = &self.shared.series_switch {
            switch.store(enabled, Ordering::SeqCst);
            let _ = self.tx.send(Control::Wake);
        }
    }

    /// What Find learned from `addr` in the last 5 minutes, if anything.
    pub fn cached_find(&self, addr: SocketAddr) -> Option<CalibreInfo> {
        let cache = self.shared.find_cache.lock().unwrap();
        cache
            .get(&addr)
            .filter(|(at, _)| at.elapsed() < FIND_CACHE_FOR)
            .map(|(_, info)| info.clone())
    }

    /// Records what a server answered, for [`Handle::cached_find`].
    pub fn remember_find(&self, addr: SocketAddr, info: CalibreInfo) {
        let mut cache = self.shared.find_cache.lock().unwrap();
        cache.retain(|_, (at, _)| at.elapsed() < FIND_CACHE_FOR);
        cache.insert(addr, (Instant::now(), info));
    }

    /// Tells the Find cache what a broadcast found: an address answering
    /// on another port loses its old entries (ADR 0021).
    pub fn note_servers(&self, servers: &[CalibreServer]) {
        self.shared.note_servers(servers);
    }

    pub fn wifi(&self) -> bool {
        self.shared.wifi.load(Ordering::SeqCst)
    }

    /// The linked library's sync settings changed: end the session once
    /// Calibre is quiet and reconnect, so they take effect (ADR 0017).
    pub fn settings_changed(&self) {
        self.shared.settings_changed.store(true, Ordering::SeqCst);
    }

    pub fn cancel_check(&self) {
        self.shared.checker.cancel();
    }

    pub fn check_status(&self) -> Option<CheckStatus> {
        self.shared.checker.status()
    }

    /// The library that passed the latest check, ready to link.
    pub fn checked(&self) -> Option<Checked> {
        self.shared.checker.checked()
    }
}

/// Finds Calibre servers: see [`Options::discover`].
pub type Discover = dyn FnMut(&[IpAddr]) -> Vec<CalibreServer> + Send;

pub struct Options {
    pub device: DeviceConfig,
    pub state_dir: PathBuf,
    /// Delays between retries; the last one repeats.
    pub backoff: Vec<Duration>,
    /// Silence on a session this long means the connection is dead.
    pub read_timeout: Duration,
    /// Finds Calibre servers by broadcast, also sending to each of the
    /// given addresses.
    pub discover: Box<Discover>,
    /// Asks one address directly, and briefly, whether Calibre still
    /// answers there (ADR 0021).
    pub ask: Box<dyn FnMut(IpAddr) -> Vec<CalibreServer> + Send>,
    /// How the service waits for a Calibre that quit (ADR 0021).
    pub away: Away,
    /// The device name to give Calibre, read before each session so a
    /// rename on the Kindle shows at the next connect (ADR 0015).
    pub device_name: Box<dyn FnMut() -> String + Send>,
    /// How long Calibre must be quiet before a session or a check may leave
    /// without an error dialog: [`session::QUIET_BEFORE_RECONNECT`].
    pub quiet: Duration,
    /// How long Disconnect and the like wait for a keep-alive before cutting
    /// the connection: [`LEAVE_WITHIN`].
    pub leave_within: Duration,
}

/// How long the user's Disconnect waits for Calibre's next keep-alive before
/// cutting the connection anyway (ADR 0022). Idle, Calibre sends one about
/// every 10 s.
pub const LEAVE_WITHIN: Duration = Duration::from_secs(15);

impl Options {
    pub fn default_backoff() -> Vec<Duration> {
        [10, 30, 120].into_iter().map(Duration::from_secs).collect()
    }
}

/// Telling Calibre quitting from a user's eject, and waiting for it to
/// come back (ADR 0021).
#[derive(Debug, Clone)]
pub struct Away {
    /// How long a quitting Calibre takes to close its sockets, before it's
    /// asked whether it's still there.
    pub settle: Duration,
    /// While it's away, broadcast this often...
    pub every: Duration,
    /// ...for this long, then every `then`.
    pub fast_for: Duration,
    pub then: Duration,
}

impl Default for Away {
    fn default() -> Away {
        Away {
            settle: Duration::from_secs(2),
            every: Duration::from_secs(10),
            fast_for: Duration::from_secs(5 * 60),
            then: Duration::from_secs(60),
        }
    }
}

/// What Home says under the status while Calibre is away (ADR 0021); a
/// newline breaks the line.
pub const CALIBRE_CLOSED: &str =
    "Can't connect to Calibre.\nTap Connect below to retry immediately.";
/// What Home says under the status after an eject.
pub const EJECTED: &str = "Ejected by Calibre.\nTap Connect below to reconnect.";

pub struct Daemon {
    lib: Library,
    opts: Options,
    rx: Receiver<Control>,
    shared: Arc<Shared>,
    net_up: bool,
    paused: bool,
    /// Addresses that answered a broadcast since Wi-Fi came up: only their
    /// silence after an eject means Calibre quit (ADR 0021).
    heard: HashSet<IpAddr>,
}

enum Wake {
    Network(bool),
    ConnectNow,
    Pause,
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
    /// Calibre at this address ejected us: a user's eject, or quitting.
    Ejected(SocketAddr),
    PasswordRejected(String),
    Lost(String),
    /// The library's sync columns changed: reconnect at once (ADR 0017).
    Reconnect,
    /// A check is using the library's address: wait until it's done.
    Held,
    /// Calibre closed the connection before saying anything.
    LockedOut,
}

impl Daemon {
    pub fn new(lib: Library, opts: Options, net_up: bool) -> (Daemon, Handle) {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            status: Mutex::new(Status {
                state: State::Stopped,
                detail: String::new(),
                since: 0,
                retry_at: None,
                advice: None,
            }),
            active: Mutex::new(None),
            status_file: opts.state_dir.join(STATUS_FILE),
            found: Mutex::new(Vec::new()),
            checker: Checker::default(),
            state_dir: opts.state_dir.clone(),
            books_dir: lib.books_dir().to_path_buf(),
            device: Mutex::new(opts.device.clone()),
            quiet: opts.quiet,
            hold: Mutex::new(None),
            held: AtomicBool::new(false),
            wifi: AtomicBool::new(net_up),
            settings_changed: AtomicBool::new(false),
            series_switch: lib.series_switch(),
            session: AtomicU64::new(0),
            leaving: AtomicBool::new(false),
            leave_within: opts.leave_within,
            find_cache: Mutex::new(HashMap::new()),
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
                paused: false,
                heard: HashSet::new(),
            },
            handle,
        )
    }

    fn set(&self, state: State, detail: impl Into<String>) {
        self.shared.set(state, detail.into());
    }

    /// Broadcasts for Calibre servers, also asking `also` directly, and
    /// notes who answered.
    fn discover(&mut self, also: &[IpAddr]) -> Vec<CalibreServer> {
        let found = (self.opts.discover)(also);
        self.heard.extend(found.iter().map(|s| s.addr));
        self.shared.note_servers(&found);
        found
    }

    /// Whether Calibre at `ip` still answers (ADR 0021).
    fn ask(&mut self, ip: IpAddr) -> bool {
        let found: Vec<CalibreServer> = (self.opts.ask)(ip)
            .into_iter()
            .filter(|s| s.addr == ip)
            .collect();
        self.shared.note_servers(&found);
        !found.is_empty()
    }

    /// Waits for a control message. Pause and resume are applied here, so
    /// every wait honours them; resuming counts as "Connect now".
    fn wait(&mut self, timeout: Option<Duration>) -> Wake {
        // While idle, also run the series pass when it falls due (ADR 0016).
        let deadline = timeout.map(|t| Instant::now() + t);
        let msg = loop {
            let left = deadline.map(|d| d.saturating_duration_since(Instant::now()));
            let step = match (left, self.lib.next_work_in()) {
                (Some(l), Some(s)) => Some(l.min(s)),
                (l, s) => l.or(s),
            };
            let received = match step {
                Some(t) => self.rx.recv_timeout(t),
                None => self.rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match received {
                Ok(Control::Wake) => self.lib.reconcile_series_if_due(),
                Ok(m) => break m,
                Err(RecvTimeoutError::Disconnected) => return Wake::Shutdown,
                Err(RecvTimeoutError::Timeout) => {
                    self.lib.reconcile_series_if_due();
                    if deadline.is_some_and(|d| Instant::now() >= d) {
                        return Wake::Timeout;
                    }
                }
            }
        };
        match msg {
            Control::Network(up) => Wake::Network(up),
            Control::ConnectNow => Wake::ConnectNow,
            Control::Pause => {
                self.paused = true;
                Wake::Pause
            }
            Control::Resume => {
                self.paused = false;
                Wake::ConnectNow
            }
            Control::Shutdown => Wake::Shutdown,
            Control::Wake => unreachable!("handled while waiting"),
        }
    }

    /// Runs until [`Handle::shutdown`].
    pub fn run(mut self) {
        // Reset whenever Wi-Fi cycles (ADR 0008).
        let mut ejected = false;
        // Calibre at this address quit, and since when: wait for its
        // broadcast answers before connecting (ADR 0021).
        let mut away: Option<(IpAddr, Instant)> = None;
        // Addresses serving a library other than the linked one, and which
        // library was linked when they were added: linking another library
        // makes them worth trying again, as do a Wi-Fi cycle and Connect now
        // (Calibre may have switched back).
        // Each with the library it served, for the status.
        let mut skip: HashMap<SocketAddr, String> = HashMap::new();
        let mut skip_for = String::new();
        // The library that rejected our password, until the user acts.
        let mut bad_password: Option<String> = None;
        let mut attempt = 0usize;

        loop {
            if self.paused {
                self.set(State::Paused, "Sync paused. Resume to reconnect.");
                match self.wait(None) {
                    Wake::Network(up) => {
                        if up && !self.net_up {
                            ejected = false;
                            away = None;
                            self.heard.clear();
                            skip.clear();
                            bad_password = None;
                        }
                        self.net_up = up;
                    }
                    Wake::ConnectNow => {
                        skip.clear();
                        ejected = false;
                        away = None;
                        bad_password = None;
                        attempt = 0;
                    }
                    Wake::Shutdown => break,
                    _ => {}
                }
                continue;
            }
            if !self.net_up {
                self.set(State::WaitingForWifi, "Waiting for Wi-Fi.");
                match self.wait(None) {
                    Wake::Network(true) => {
                        self.net_up = true;
                        ejected = false;
                        away = None;
                        self.heard.clear();
                        skip.clear();
                        bad_password = None;
                        attempt = 0;
                    }
                    Wake::Shutdown => break,
                    _ => {}
                }
                continue;
            }
            if ejected {
                self.shared.set_full(
                    State::Ejected,
                    "Ejected by Calibre. Will reconnect when Wi-Fi reconnects, or with Connect now."
                        .into(),
                    Some(EJECTED.into()),
                    None,
                );
                match self.wait(None) {
                    Wake::Network(false) => self.net_up = false,
                    Wake::ConnectNow => {
                        skip.clear();
                        ejected = false;
                        away = None;
                        bad_password = None;
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
            let Some(library) = config.library else {
                self.set(
                    State::NotLinked,
                    "No Calibre library is linked yet. Use Find libraries, or link one by address.",
                );
                match self.wait(Some(NO_LIBRARY_RECHECK)) {
                    Wake::Network(false) => self.net_up = false,
                    Wake::Shutdown => break,
                    _ => {}
                }
                continue;
            };
            if bad_password.as_deref() == Some(library.uuid.as_str()) {
                // The library rejected our password; wait for the user
                // (Connect now after fixing it) or a Wi-Fi cycle.
                match self.wait(None) {
                    Wake::Network(false) => self.net_up = false,
                    Wake::ConnectNow => {
                        skip.clear();
                        bad_password = None;
                        attempt = 0;
                    }
                    Wake::Shutdown => break,
                    _ => {}
                }
                continue;
            }

            if skip_for != library.uuid {
                skip.clear();
                skip_for = library.uuid.clone();
            }

            // Our state may still belong to a library linked before.
            if let Err(e) = self.lib.reset_if_relinked(&library.uuid) {
                self.set(
                    State::Error,
                    format!("Cannot reset for the newly linked library: {e}"),
                );
                match self.wait(Some(NO_LIBRARY_RECHECK)) {
                    Wake::Network(false) => self.net_up = false,
                    Wake::Shutdown => break,
                    _ => {}
                }
                continue;
            }

            let name = library.display_name().to_owned();
            self.set(State::Searching, format!("Looking for {name}."));
            // Also ask the library's last address directly: Calibre answers
            // the sender, so this crosses routers, where broadcasts don't
            // (ADR 0021, amendment).
            let mut also: Vec<IpAddr> = away.map(|(ip, _)| ip).into_iter().collect();
            if let Some(ip) = last_ip(&library).filter(|ip| !also.contains(ip)) {
                also.push(ip);
            }
            let found = self.discover(&also);
            if let Some((ip, since)) = away {
                if found.iter().any(|s| s.addr == ip || serves(&library, s)) {
                    log::info!("Calibre is back");
                    away = None;
                    attempt = 0;
                } else {
                    // Nothing to connect to until it answers (ADR 0021).
                    let a = &self.opts.away;
                    let delay = if since.elapsed() < a.fast_for {
                        a.every
                    } else {
                        a.then
                    };
                    self.shared.set_retrying(
                        State::NotFound,
                        format!("Calibre has closed. Looking again in {}s.", delay.as_secs()),
                        Some(CALIBRE_CLOSED.into()),
                        delay,
                    );
                    match self.wait(Some(delay)) {
                        Wake::Network(false) => self.net_up = false,
                        Wake::ConnectNow => {
                            away = None;
                            skip.clear();
                            bad_password = None;
                            attempt = 0;
                        }
                        Wake::Shutdown => break,
                        Wake::Network(true) | Wake::Timeout | Wake::Pause => {}
                    }
                    continue;
                }
            }
            let candidates = candidates(&library, &found, &skip);

            let mut outcome = Outcome::NotFound;
            for cand in candidates {
                if self.shared.is_held(cand.addr) {
                    outcome = Outcome::Held;
                    continue;
                }
                match self.try_session(&cand, &library) {
                    Err(e) => log::debug!("cannot reach {}: {e}", cand.addr),
                    Ok((end, info)) => {
                        let name = info
                            .as_ref()
                            .map(|i| i.library_name.clone())
                            .unwrap_or_else(|| name.clone());
                        match end {
                            Ok(SessionEnd::Ejected) => {
                                outcome = Outcome::Ejected(cand.addr);
                                break;
                            }
                            // Another library at this address: from the
                            // start, or switched to in Calibre meanwhile.
                            Ok(SessionEnd::LibraryRejected(info))
                            | Ok(SessionEnd::LibraryChanged(info)) => {
                                log::warn!(
                                    "{} now serves library {:?}, which is not linked",
                                    cand.addr,
                                    info.library_name
                                );
                                self.shared.record_found(FoundLibrary::new(
                                    &info,
                                    cand.addr.ip().to_string(),
                                    cand.addr.port(),
                                    cand.hostname.clone(),
                                ));
                                skip.insert(cand.addr, info.library_name.clone());
                            }
                            Ok(SessionEnd::PasswordRejected) => {
                                bad_password = info.as_ref().map(|i| i.library_uuid.clone());
                                outcome = Outcome::PasswordRejected(name);
                                break;
                            }
                            Ok(SessionEnd::Busy) => outcome = Outcome::Busy,
                            // Left for Disconnect and the like: the
                            // control message that asked is waiting.
                            Ok(SessionEnd::ColumnsChanged | SessionEnd::Left) => {
                                outcome = Outcome::Reconnect;
                                break;
                            }
                            Ok(SessionEnd::Yielded) => {
                                outcome = Outcome::Held;
                                break;
                            }
                            Ok(SessionEnd::ClosedAtOnce) => {
                                outcome = Outcome::LockedOut;
                                break;
                            }
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
            let mut connect_now = false;
            while let Ok(msg) = self.rx.try_recv() {
                match msg {
                    Control::Network(false) => {
                        went_down = true;
                        self.net_up = false;
                    }
                    Control::Network(true) => self.net_up = true,
                    Control::ConnectNow => {
                        skip.clear();
                        away = None;
                        bad_password = None;
                        attempt = 0;
                        connect_now = true;
                    }
                    Control::Pause => self.paused = true,
                    Control::Resume => {
                        self.paused = false;
                        attempt = 0;
                    }
                    Control::Shutdown => shutdown = true,
                    Control::Wake => {}
                }
            }
            if shutdown {
                break;
            }
            if self.paused {
                continue;
            }
            if went_down {
                if self.net_up {
                    // Down and back up already: a Wi-Fi cycle.
                    ejected = false;
                    away = None;
                    self.heard.clear();
                    skip.clear();
                    bad_password = None;
                    attempt = 0;
                }
                continue;
            }

            let delay = self.opts.backoff[attempt.min(self.opts.backoff.len() - 1)];
            let secs = delay.as_secs();
            match outcome {
                // Unlink, a new password or Connect now cut the session:
                // go round again at once, without a "lost" status.
                Outcome::Lost(_) | Outcome::NotFound | Outcome::Busy if connect_now => continue,
                Outcome::Reconnect => {
                    attempt = 0;
                    continue;
                }
                Outcome::Ejected(addr) => {
                    attempt = 0;
                    let ip = addr.ip();
                    if !self.heard.contains(&ip) {
                        // Calibre there never answered a broadcast, so its
                        // silence would prove nothing (ADR 0021).
                        ejected = true;
                        continue;
                    }
                    // A user's eject, or Calibre quitting? A quitting
                    // Calibre stops answering broadcasts.
                    self.set(
                        State::Searching,
                        "Disconnected by Calibre. Checking whether Calibre is still open.",
                    );
                    match self.wait(Some(self.opts.away.settle)) {
                        Wake::Network(false) => {
                            self.net_up = false;
                            continue;
                        }
                        Wake::Shutdown => break,
                        // Connect now, or Pause (resuming connects anyway).
                        Wake::ConnectNow | Wake::Pause => continue,
                        Wake::Network(true) | Wake::Timeout => {}
                    }
                    if self.ask(ip) {
                        ejected = true;
                    } else {
                        log::info!("Calibre at {ip} no longer answers: it quit");
                        self.shared.forget_address(ip);
                        away = Some((ip, Instant::now()));
                    }
                    continue;
                }
                Outcome::Held => {
                    // Releasing the hold sends Connect now. Set first, so a
                    // release between here and the check below isn't missed.
                    self.shared.held.store(true, Ordering::SeqCst);
                    if self.shared.hold.lock().unwrap().is_none() {
                        // Ended for new settings, or the check is done.
                        self.shared.held.store(false, Ordering::SeqCst);
                        attempt = 0;
                        continue;
                    }
                    self.set(
                        State::Searching,
                        format!("Waiting while Calibre at the address of {name} is checked."),
                    );
                    match self.wait(Some(Duration::from_secs(120))) {
                        Wake::Network(false) => self.net_up = false,
                        Wake::Shutdown => break,
                        _ => {}
                    }
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
                Outcome::NotFound => match skip.iter().next() {
                    // Calibre is there, serving another library: say so.
                    Some((addr, other)) => {
                        let advice = format!(
                            "Calibre at {} is serving {other}, not {name}. Switch back to {name} \
                             in Calibre, then use Connect.",
                            addr.ip()
                        );
                        self.shared.set_retrying(
                            State::NotFound,
                            advice.clone(),
                            Some(advice),
                            delay,
                        )
                    }
                    None => self.shared.set_retrying(
                        State::NotFound,
                        format!("Calibre library {name} not found. Retrying in {secs}s."),
                        None,
                        delay,
                    ),
                },
                Outcome::Busy => self.shared.set_retrying(
                    State::NotFound,
                    format!("Calibre is busy with another device. Retrying in {secs}s."),
                    Some("Calibre is busy with another device.".into()),
                    delay,
                ),
                Outcome::Lost(name) => self.shared.set_retrying(
                    State::NotFound,
                    format!("Lost the connection to {name}. Retrying in {secs}s."),
                    Some(format!("Lost the connection to {name}.")),
                    delay,
                ),
                Outcome::LockedOut => self.shared.set_retrying(
                    State::LockedOut,
                    format!("{} Retrying in {secs}s.", check::LOCKED_OUT),
                    Some(check::LOCKED_OUT.into()),
                    delay,
                ),
            }
            attempt += 1;
            match self.wait(Some(delay)) {
                Wake::Network(false) => self.net_up = false,
                Wake::ConnectNow => {
                    skip.clear();
                    bad_password = None;
                    attempt = 0;
                }
                Wake::Shutdown => break,
                Wake::Network(true) | Wake::Timeout | Wake::Pause => {}
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
        library: &LinkedLibrary,
    ) -> io::Result<(io::Result<SessionEnd>, Option<CalibreInfo>)> {
        let stream = TcpStream::connect_timeout(&cand.addr, CONNECT_TIMEOUT)?;
        self.opts.device.device_name = (self.opts.device_name)();
        *self.shared.device.lock().unwrap() = self.opts.device.clone();
        stream.set_read_timeout(Some(self.opts.read_timeout))?;
        stream.set_nodelay(true)?;
        {
            let mut active = self.shared.active.lock().unwrap();
            self.shared.session.fetch_add(1, Ordering::SeqCst);
            self.shared.leaving.store(false, Ordering::SeqCst);
            *active = Some(stream.try_clone()?);
        }
        let mut conn = Connection::new(BufReader::new(stream.try_clone()?), BufWriter::new(stream));

        let mut admit = LinkedAdmit {
            library,
            addr: cand.addr,
            shared: &self.shared,
            state_dir: &self.opts.state_dir,
            answered: None,
            admitted: false,
        };
        let end = session::run_with_quiet(
            &mut conn,
            &mut self.lib,
            &self.opts.device,
            &mut admit,
            self.opts.quiet,
        );
        let LinkedAdmit {
            answered, admitted, ..
        } = admit;
        *self.shared.active.lock().unwrap() = None;

        if admitted {
            if let Some(info) = &answered {
                self.record_connected(info, cand);
            }
        }
        Ok((end, answered))
    }

    /// Remembers where the linked library answered. Runs when the session
    /// ends, so the library may have been unlinked meanwhile: never re-link it.
    fn record_connected(&self, info: &CalibreInfo, cand: &Candidate) {
        let result = config::update(&self.opts.state_dir, |c| {
            let Some(previous) = c.linked(&info.library_uuid).cloned() else {
                return;
            };
            c.link(
                LinkedLibrary {
                    uuid: info.library_uuid.clone(),
                    name: info.library_name.clone(),
                    host: Some(cand.addr.ip().to_string()),
                    port: Some(cand.addr.port()),
                    hostname: (!cand.hostname.is_empty())
                        .then(|| cand.hostname.clone())
                        .or(previous.hostname),
                    password: None,
                    ..Default::default()
                },
                false,
            );
        });
        if let Err(e) = result {
            log::warn!("cannot update settings: {e}");
        }
    }
}

/// Admits only the linked library, and keeps its columns in `config.json`
/// (ADR 0020).
struct LinkedAdmit<'a> {
    library: &'a LinkedLibrary,
    addr: SocketAddr,
    shared: &'a Shared,
    state_dir: &'a Path,
    answered: Option<CalibreInfo>,
    admitted: bool,
}

impl Admit for LinkedAdmit<'_> {
    fn admit(&mut self, info: &CalibreInfo) -> Admission {
        self.answered = Some(info.clone());
        if info.library_uuid != self.library.uuid {
            return Admission::Reject;
        }
        self.admitted = true;
        self.shared.set(
            State::Connected,
            format!("Connected to {}.", info.library_name),
        );
        Admission::Accept {
            password: self.library.password.clone(),
            columns: self.library.sync_columns(),
            known_columns: self.library.columns.clone(),
        }
    }

    fn columns_seen(&mut self, columns: &Map<String, Value>) -> io::Result<()> {
        let uuid = &self.library.uuid;
        config::update(self.state_dir, |c| c.set_columns(uuid, columns.clone())).map(|_| ())
    }

    fn end_when_quiet(&mut self) -> bool {
        self.shared.is_held(self.addr) || self.shared.settings_changed.swap(false, Ordering::SeqCst)
    }

    fn leave_now(&mut self) -> bool {
        self.shared.leaving.load(Ordering::SeqCst)
    }
}

/// Addresses worth connecting to in the background: discovered servers that
/// match the linked library, then its last address.
fn candidates(
    library: &LinkedLibrary,
    found: &[CalibreServer],
    skip: &HashMap<SocketAddr, String>,
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut push = |addr: SocketAddr, hostname: String| {
        if !skip.contains_key(&addr) && !out.iter().any(|c| c.addr == addr) {
            out.push(Candidate { addr, hostname });
        }
    };
    for s in found.iter().filter(|s| serves(library, s)) {
        push(SocketAddr::new(s.addr, s.port), s.hostname.clone());
    }
    if let (Some(host), Some(port)) = (&library.host, library.port) {
        match (host.as_str(), port).to_socket_addrs() {
            Ok(mut addrs) => {
                if let Some(addr) = addrs.next() {
                    push(addr, library.hostname.clone().unwrap_or_default());
                }
            }
            Err(e) => log::debug!("cannot resolve {host}: {e}"),
        }
    }
    out
}

/// The address the linked library last answered at, if it has one.
fn last_ip(library: &LinkedLibrary) -> Option<IpAddr> {
    let host = library.host.as_deref()?;
    host.parse().ok().or_else(|| {
        let mut addrs = (host, library.port.unwrap_or(0)).to_socket_addrs().ok()?;
        addrs.next().map(|a| a.ip())
    })
}

/// Whether a discovered server is where the linked library was: the same
/// hostname, or the same address.
fn serves(library: &LinkedLibrary, s: &CalibreServer) -> bool {
    library.hostname.as_deref() == Some(s.hostname.as_str())
        || library.host.as_deref() == Some(s.addr.to_string().as_str())
}

/// Why [`probe`] failed, when it's Calibre's doing rather than the network's.
#[derive(Debug)]
enum ProbeError {
    Busy,
    /// Closed before sending anything: most likely Calibre's lockout.
    LockedOut,
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ProbeError::Busy => "Calibre is busy with another device",
            ProbeError::LockedOut => check::LOCKED_OUT,
        })
    }
}

impl std::error::Error for ProbeError {}

fn probe_error(e: &io::Error) -> Option<&ProbeError> {
    e.get_ref()
        .and_then(|inner| inner.downcast_ref::<ProbeError>())
}

/// Whether a [`probe`] error means Calibre is serving another device.
pub fn is_busy(e: &io::Error) -> bool {
    matches!(probe_error(e), Some(ProbeError::Busy))
}

/// Whether a [`probe`] error means Calibre closed the connection at once,
/// most likely its lockout after too many failed connections (ADR 0020).
pub fn is_locked_out(e: &io::Error) -> bool {
    matches!(probe_error(e), Some(ProbeError::LockedOut))
}

/// Connects to `addr`, reads which library Calibre serves, and disconnects
/// without completing the handshake. Calibre counts that as a failed
/// connection (ADR 0020).
pub fn probe(addr: SocketAddr) -> io::Result<CalibreInfo> {
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    stream.set_read_timeout(Some(PROBE_READ_TIMEOUT))?;
    let mut reader = BufReader::new(stream);
    match read_message(&mut reader)? {
        Some(m) if m.opcode() == Some(Opcode::GetInitializationInfo) => {
            Ok(CalibreInfo::from_init_payload(&m.payload))
        }
        Some(m) if m.opcode() == Some(Opcode::CalibreBusy) => {
            Err(io::Error::other(ProbeError::Busy))
        }
        None => Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            ProbeError::LockedOut,
        )),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a Calibre wireless device connection",
        )),
    }
}

/// What "Find libraries" learned about one discovered server.
pub enum FindResult {
    /// The linked library's server; not contacted.
    Linked,
    Answered(CalibreInfo),
    /// Answered recently: reused without connecting (ADR 0020).
    Cached(CalibreInfo),
    Failed(io::Error),
}

/// "Find libraries": probes each discovered server that isn't the linked
/// library's (ADR 0008), unless `cached` knows what it answered recently
/// (ADR 0020). Remembering what answered is up to the caller: the daemon
/// keeps it in memory (ADR 0018).
pub fn find_libraries(
    config: &Config,
    found: &[CalibreServer],
    cached: &dyn Fn(SocketAddr) -> Option<CalibreInfo>,
) -> Vec<(CalibreServer, FindResult)> {
    found
        .iter()
        .map(|s| {
            let addr = SocketAddr::new(s.addr, s.port);
            let result = if config.matches_linked(&s.hostname, &s.addr.to_string()) {
                FindResult::Linked
            } else if let Some(info) = cached(addr) {
                FindResult::Cached(info)
            } else {
                match probe(addr) {
                    Ok(info) => FindResult::Answered(info),
                    Err(e) => FindResult::Failed(e),
                }
            };
            (s.clone(), result)
        })
        .collect()
}

/// Why [`link_address`] didn't link.
#[derive(Debug)]
pub enum LinkError {
    /// Nothing answered as Calibre at that address.
    Unreachable(io::Error),
    /// A different library answered than the one asked for.
    OtherLibrary(CalibreInfo),
    /// Another library is linked, and replacing it wasn't confirmed.
    NeedsReplace {
        linked: Box<LinkedLibrary>,
        answered: CalibreInfo,
    },
    Settings(io::Error),
}

/// Links the library at `host:port` ("Add server", or `link`): probes it now,
/// and links the library that answers with this address. With
/// `expect_uuid`, refuses any other library. On failure nothing is stored
/// (ADR 0008, ADR 0018).
pub fn link_address(
    state_dir: &Path,
    host: &str,
    port: u16,
    password: Option<String>,
    expect_uuid: Option<&str>,
    replace: bool,
) -> Result<(CalibreInfo, Link), LinkError> {
    let addr = (host, port)
        .to_socket_addrs()
        .and_then(|mut a| {
            a.next().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("cannot resolve {host}"))
            })
        })
        .map_err(LinkError::Unreachable)?;
    let info = probe(addr).map_err(LinkError::Unreachable)?;
    if expect_uuid.is_some_and(|u| u != info.library_uuid) {
        return Err(LinkError::OtherLibrary(info));
    }
    let lib = LinkedLibrary {
        uuid: info.library_uuid.clone(),
        name: info.library_name.clone(),
        host: Some(host.to_owned()),
        port: Some(port),
        hostname: None,
        password,
        ..Default::default()
    };
    match config::update(state_dir, |c| c.link(lib, replace)) {
        Err(e) => Err(LinkError::Settings(e)),
        Ok(Link::Refused(linked)) => Err(LinkError::NeedsReplace {
            linked: Box::new(linked),
            answered: info,
        }),
        Ok(link) => Ok((info, link)),
    }
}

/// Links a library from the found list, at the address it was found at.
pub fn link_found(state_dir: &Path, found: &FoundLibrary, replace: bool) -> io::Result<Link> {
    config::update(state_dir, |c| {
        c.link(
            LinkedLibrary {
                uuid: found.uuid.clone(),
                name: found.name.clone(),
                host: Some(found.host.clone()),
                port: Some(found.port),
                hostname: (!found.hostname.is_empty()).then(|| found.hostname.clone()),
                ..Default::default()
            },
            replace,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::CheckState;
    use crate::protocol::write_message;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    /// A fake Calibre: on each connection it announces `uuid`, and if the
    /// device answers, ejects it.
    fn fake_calibre(uuid: &'static str) -> (SocketAddr, Arc<AtomicUsize>) {
        fake_calibre_with(uuid, true)
    }

    /// Like [`fake_calibre`], but with `eject: false` it keeps an admitted
    /// device connected (sending keepalives) until the device hangs up.
    fn fake_calibre_with(uuid: &'static str, eject: bool) -> (SocketAddr, Arc<AtomicUsize>) {
        let (addr, count, _) = fake_calibre_recording(uuid, eject);
        (addr, count)
    }

    /// Like [`fake_calibre_with`], also recording the `deviceName` of each
    /// handshake reply.
    #[allow(clippy::type_complexity)]
    fn fake_calibre_recording(
        uuid: &'static str,
        eject: bool,
    ) -> (SocketAddr, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let names = Arc::new(Mutex::new(Vec::new()));
        let (c, n) = (count.clone(), names.clone());
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
                let Ok(Some(reply)) = read_message(&mut reader) else {
                    continue; // device refused or probed
                };
                if let Some(name) = reply.payload["deviceName"].as_str() {
                    n.lock().unwrap().push(name.to_owned());
                }
                if eject {
                    // The library's columns, as Calibre lists them (ADR 0020).
                    let fm = json!({"fieldMetadata": {"#kindle_read":
                        {"is_custom": true, "datatype": "bool", "name": "Kindle read"}}});
                    let _ = write_message(&mut stream, Opcode::SetLibraryInfo, &fm);
                    let _ = read_message(&mut reader);
                    let _ = write_message(&mut stream, Opcode::Noop, &json!({"ejecting": true}));
                    let _ = read_message(&mut reader);
                } else {
                    // Library info and the book list, then keep-alives.
                    let fm = json!({"fieldMetadata": {}});
                    let _ = write_message(&mut stream, Opcode::SetLibraryInfo, &fm);
                    let _ = read_message(&mut reader);
                    let count = json!({"willUseCachedMetadata": true});
                    let _ = write_message(&mut stream, Opcode::GetBookCount, &count);
                    let _ = read_message(&mut reader);
                    while write_message(&mut stream, Opcode::Noop, &json!({})).is_ok()
                        && matches!(read_message(&mut reader), Ok(Some(_)))
                    {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
            }
        });
        (addr, count, names)
    }

    fn start(
        state: &Path,
        linked: Option<(&str, SocketAddr)>,
    ) -> (Handle, std::thread::JoinHandle<()>) {
        start_with(
            state,
            linked,
            Duration::from_millis(50),
            Box::new(|| "Test Kindle".into()),
        )
    }

    fn start_with_backoff(
        state: &Path,
        linked: Option<(&str, SocketAddr)>,
        backoff: Duration,
    ) -> (Handle, std::thread::JoinHandle<()>) {
        start_with(state, linked, backoff, Box::new(|| "Test Kindle".into()))
    }

    fn start_with(
        state: &Path,
        linked: Option<(&str, SocketAddr)>,
        backoff: Duration,
        device_name: Box<dyn FnMut() -> String + Send>,
    ) -> (Handle, std::thread::JoinHandle<()>) {
        launch(state, linked, |opts| {
            opts.backoff = vec![backoff];
            opts.device_name = device_name;
        })
    }

    /// Starts a daemon whose broadcasts get `answers`, as they stand at
    /// each broadcast, and that waits for a quit Calibre in milliseconds.
    fn start_answering(
        state: &Path,
        linked: (&str, SocketAddr),
        answers: Arc<Mutex<Vec<CalibreServer>>>,
    ) -> (Handle, std::thread::JoinHandle<()>) {
        launch(state, Some(linked), |opts| {
            let a = answers.clone();
            opts.discover = Box::new(move |_| a.lock().unwrap().clone());
            opts.ask = Box::new(move |ip| {
                let answers = answers.lock().unwrap();
                answers.iter().filter(|s| s.addr == ip).cloned().collect()
            });
            opts.away = Away {
                settle: Duration::from_millis(10),
                every: Duration::from_millis(50),
                fast_for: Duration::from_secs(1),
                then: Duration::from_millis(50),
            };
        })
    }

    fn launch(
        state: &Path,
        linked: Option<(&str, SocketAddr)>,
        edit: impl FnOnce(&mut Options),
    ) -> (Handle, std::thread::JoinHandle<()>) {
        let mut config = Config::load(state).unwrap();
        if let Some((uuid, addr)) = linked {
            config.link(
                LinkedLibrary {
                    uuid: uuid.into(),
                    name: "Test Library".into(),
                    host: Some(addr.ip().to_string()),
                    port: Some(addr.port()),
                    ..Default::default()
                },
                false,
            );
        }
        config.save().unwrap();
        let lib = Library::open(&state.join("books"), state).unwrap();
        let mut opts = Options {
            device: DeviceConfig::default(),
            state_dir: state.to_path_buf(),
            backoff: vec![Duration::from_millis(50)],
            read_timeout: Duration::from_secs(5),
            discover: Box::new(|_| Vec::new()),
            ask: Box::new(|_| Vec::new()),
            away: Away::default(),
            device_name: Box::new(|| "Test Kindle".into()),
            quiet: Duration::ZERO,
            leave_within: LEAVE_WITHIN,
        };
        edit(&mut opts);
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
        // Broadcasts get no answers, so an eject can't be told from a quit
        // and keeps the service disconnected (ADR 0021, rule 5).
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
    fn a_calibre_that_quit_is_waited_for_and_one_that_ejected_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let (addr, count) = fake_calibre("lib-1");
        let server = |port| CalibreServer {
            addr: addr.ip(),
            hostname: "pc".into(),
            port,
            content_server_port: None,
        };
        let answers = Arc::new(Mutex::new(vec![server(addr.port())]));
        let (h, thread) = start_answering(tmp.path(), ("lib-1", addr), answers.clone());

        // Calibre still answers after the eject: the user's eject.
        wait_until("first eject", || h.status().state == State::Ejected);
        assert_eq!(h.status().advice.as_deref(), Some(EJECTED));
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(count.load(Ordering::SeqCst), 1, "no reconnect after eject");

        // Calibre quits: it ejects us and stops answering.
        answers.lock().unwrap().clear();
        let other = SocketAddr::new(addr.ip(), 1);
        h.remember_find(other, CalibreInfo::default());
        h.connect_now();
        wait_until("Calibre closed", || {
            h.status().advice.as_deref() == Some(CALIBRE_CLOSED)
        });
        let s = h.status();
        assert_eq!(s.state, State::NotFound);
        assert!(s.retry_at.is_some());
        assert_eq!(h.cached_find(other), None, "Find forgets the address");
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(count.load(Ordering::SeqCst), 2, "no connection while away");

        // It comes back on another port: connect there, and record it.
        let (moved, moved_count) = fake_calibre("lib-1");
        *answers.lock().unwrap() = vec![server(moved.port())];
        wait_until("reconnected", || moved_count.load(Ordering::SeqCst) == 1);
        wait_until("ejected again", || h.status().state == State::Ejected);
        let config = Config::load(tmp.path()).unwrap();
        assert_eq!(config.linked("lib-1").unwrap().port, Some(moved.port()));
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn searches_also_ask_the_last_address_directly() {
        let tmp = tempfile::tempdir().unwrap();
        let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let a = asked.clone();
        let (h, thread) = launch(tmp.path(), Some(("lib-1", dead)), |opts| {
            opts.discover = Box::new(move |also| {
                a.lock().unwrap().push(also.to_vec());
                Vec::new()
            })
        });
        wait_until("a search", || !asked.lock().unwrap().is_empty());
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert_eq!(asked.lock().unwrap()[0], vec![ip]);
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn find_drops_answers_from_an_address_that_moved_port() {
        let tmp = tempfile::tempdir().unwrap();
        let (h, thread) = start(tmp.path(), None);
        let old: SocketAddr = "10.0.0.7:9090".parse().unwrap();
        let elsewhere: SocketAddr = "10.0.0.8:9090".parse().unwrap();
        h.remember_find(old, CalibreInfo::default());
        h.remember_find(elsewhere, CalibreInfo::default());
        let at = |a: &str, port| CalibreServer {
            addr: a.parse().unwrap(),
            hostname: "pc".into(),
            port,
            content_server_port: None,
        };
        h.note_servers(&[at("10.0.0.7", 9090)]);
        assert!(h.cached_find(old).is_some(), "same port: kept");
        h.note_servers(&[at("10.0.0.7", 4242)]);
        assert_eq!(h.cached_find(old), None);
        assert!(h.cached_find(elsewhere).is_some(), "silent: kept");
        h.shutdown();
        thread.join().unwrap();
    }

    /// A fake Calibre for one connection: the handshake, library info and
    /// book count, then a keep-alive every `every` (none if `None`) until
    /// the device hangs up. Sends how many keep-alives were answered, and
    /// whether the device closed the connection cleanly.
    fn fake_calibre_keeping_alive(
        every: Option<Duration>,
    ) -> (SocketAddr, Receiver<(usize, bool)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let init = json!({"serverProtocolVersion": 1, "currentLibraryUUID": "lib-1",
                              "currentLibraryName": "Test Library", "passwordChallenge": ""});
            write_message(&mut stream, Opcode::GetInitializationInfo, &init).unwrap();
            read_message(&mut reader).unwrap();
            write_message(
                &mut stream,
                Opcode::SetLibraryInfo,
                &json!({"fieldMetadata": {}}),
            )
            .unwrap();
            read_message(&mut reader).unwrap();
            let count = json!({"willUseCachedMetadata": true});
            write_message(&mut stream, Opcode::GetBookCount, &count).unwrap();
            read_message(&mut reader).unwrap();
            let Some(every) = every else {
                let clean = matches!(read_message(&mut reader), Ok(None));
                let _ = tx.send((0, clean));
                return;
            };
            let mut answered = 0;
            loop {
                std::thread::sleep(every);
                if write_message(&mut stream, Opcode::Noop, &json!({})).is_err() {
                    let _ = tx.send((answered, false));
                    return;
                }
                match read_message(&mut reader) {
                    Ok(Some(_)) => answered += 1,
                    other => {
                        let _ = tx.send((answered, matches!(other, Ok(None))));
                        return;
                    }
                }
            }
        });
        (addr, rx)
    }

    #[test]
    fn disconnect_leaves_at_a_keep_alive_without_answering_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (addr, ended) = fake_calibre_keeping_alive(Some(Duration::from_millis(400)));
        let (h, thread) = start(tmp.path(), Some(("lib-1", addr)));
        wait_until("connected", || h.status().state == State::Connected);
        std::thread::sleep(Duration::from_millis(500));

        h.pause();
        assert_eq!(h.status().state, State::Disconnecting);
        wait_until("paused", || h.status().state == State::Paused);
        // The fallback cut (15 s) can't have happened yet: it left at a
        // keep-alive, which it didn't answer, and closed cleanly.
        let (answered, clean) = ended.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(answered >= 1, "keep-alives before Disconnect were answered");
        assert!(clean);
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn disconnect_cuts_the_connection_when_no_keep_alive_comes() {
        let tmp = tempfile::tempdir().unwrap();
        let (addr, ended) = fake_calibre_keeping_alive(None);
        let (h, thread) = launch(tmp.path(), Some(("lib-1", addr)), |opts| {
            opts.leave_within = Duration::from_millis(200);
        });
        wait_until("connected", || h.status().state == State::Connected);
        let asked = Instant::now();
        h.pause();
        wait_until("paused", || h.status().state == State::Paused);
        // Well before the 5 s read timeout.
        assert!(
            asked.elapsed() < Duration::from_secs(2),
            "{:?}",
            asked.elapsed()
        );
        assert!(ended.recv_timeout(Duration::from_secs(5)).is_ok());
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn pause_disconnects_and_resume_reconnects() {
        let tmp = tempfile::tempdir().unwrap();
        let (addr, count) = fake_calibre_with("lib-1", false);
        let (h, thread) = start(tmp.path(), Some(("lib-1", addr)));

        wait_until("connected", || h.status().state == State::Connected);
        h.pause();
        wait_until("paused", || h.status().state == State::Paused);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(count.load(Ordering::SeqCst), 1, "no reconnect while paused");
        // Wi-Fi changes while paused don't resume syncing.
        h.network_changed(false);
        h.network_changed(true);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(h.status().state, State::Paused);

        h.resume();
        wait_until("reconnected", || {
            count.load(Ordering::SeqCst) == 2 && h.status().state == State::Connected
        });
        h.reconnect();
        wait_until("reconnect", || count.load(Ordering::SeqCst) == 3);
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn device_name_is_read_for_each_session() {
        let tmp = tempfile::tempdir().unwrap();
        let (addr, count, names) = fake_calibre_recording("lib-1", true);
        let mut n = 0;
        let (h, thread) = start_with(
            tmp.path(),
            Some(("lib-1", addr)),
            Duration::from_millis(50),
            Box::new(move || {
                n += 1;
                format!("Kindle {n}")
            }),
        );
        wait_until("first eject", || h.status().state == State::Ejected);
        h.connect_now();
        wait_until("second session", || count.load(Ordering::SeqCst) == 2);
        wait_until("second eject", || names.lock().unwrap().len() == 2);
        assert_eq!(*names.lock().unwrap(), ["Kindle 1", "Kindle 2"]);
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn unlink_during_a_session_sticks() {
        let tmp = tempfile::tempdir().unwrap();
        let (addr, count) = fake_calibre_with("lib-1", false);
        // A backoff longer than wait_until's deadline: the unlink must not wait it out.
        let (h, thread) =
            start_with_backoff(tmp.path(), Some(("lib-1", addr)), Duration::from_secs(60));

        wait_until("connected", || h.status().state == State::Connected);
        // What the API's /unlink does.
        assert!(config::update(tmp.path(), |c| c.unlink()).unwrap());
        h.reconnect();
        wait_until("not linked", || h.status().state == State::NotLinked);
        assert_eq!(Config::load(tmp.path()).unwrap().library, None);
        assert_eq!(count.load(Ordering::SeqCst), 1, "no reconnect after unlink");
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn other_library_at_linked_address_is_found_and_skipped() {
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
        let found = h.found();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].uuid, "someone-else");
        assert_eq!(found[0].port, addr.port());
        // In memory only.
        let config = fs::read_to_string(tmp.path().join(config::CONFIG_FILE)).unwrap();
        assert!(!config.contains("someone-else"));

        h.network_changed(false);
        h.network_changed(true);
        wait_until("retry after wifi cycle", || {
            count.load(Ordering::SeqCst) == 2
        });
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn linking_the_library_at_a_skipped_address_tries_it_again() {
        let tmp = tempfile::tempdir().unwrap();
        let (addr, count) = fake_calibre("lib-2");
        let (h, thread) = start(tmp.path(), Some(("lib-1", addr)));
        wait_until("not found", || h.status().state == State::NotFound);
        assert_eq!(h.found()[0].uuid, "lib-2");

        // What the app's Link does for a found library.
        let found = h.found().remove(0);
        assert!(matches!(
            link_found(tmp.path(), &found, true).unwrap(),
            Link::Replaced(_)
        ));
        h.reconnect();
        wait_until("connected to the newly linked library", || {
            count.load(Ordering::SeqCst) == 2 && h.status().state == State::Ejected
        });
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn nothing_linked_contacts_nobody() {
        let tmp = tempfile::tempdir().unwrap();
        let (h, thread) = start(tmp.path(), None);
        wait_until("not linked", || h.status().state == State::NotLinked);
        assert!(h.status().detail.contains("Find libraries"));
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn linking_a_different_library_resets_before_the_session() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path();
        fs::create_dir_all(state.join("books")).unwrap();
        fs::write(state.join("books/Book.txt"), b"book").unwrap();
        fs::write(
            state.join("books.json"),
            json!([{"lpath": "Book.txt", "uuid": "u1", "title": "Book", "tags": ["x"]}])
                .to_string(),
        )
        .unwrap();
        fs::write(
            state.join("driveinfo.json"),
            json!({"device_store_uuid": "dev-1", "last_library_uuid": "lib-old"}).to_string(),
        )
        .unwrap();
        let (addr, _count) = fake_calibre("lib-1");
        let (h, thread) = start(state, Some(("lib-1", addr)));
        wait_until("eject", || h.status().state == State::Ejected);
        h.shutdown();
        thread.join().unwrap();

        assert!(!state.join("driveinfo.json").exists());
        let books: serde_json::Value =
            serde_json::from_slice(&fs::read(state.join("books.json")).unwrap()).unwrap();
        assert_eq!(books[0]["uuid"], "u1");
        assert!(books[0].get("tags").is_none());
        // The session kept the library's columns with the link.
        let config = Config::load(state).unwrap();
        let columns = &config.linked("lib-1").unwrap().columns;
        assert_eq!(columns["#kindle_read"]["name"], "Kindle read");
    }

    fn check_env(state: &Path) -> check::Env {
        fs::create_dir_all(state.join("books")).unwrap();
        check::Env {
            state_dir: state.to_path_buf(),
            books_dir: state.join("books"),
            device: DeviceConfig::default(),
            quiet: Duration::ZERO,
            busy_wait: Duration::from_millis(20),
        }
    }

    #[test]
    fn a_check_tries_again_while_calibre_is_busy() {
        let tmp = tempfile::tempdir().unwrap();
        let env = check_env(tmp.path());
        // Busy at first, as Calibre is until it notices the service left.
        let (real, count) = fake_calibre("lib-1");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for (n, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else { continue };
                if n < 2 {
                    let _ = write_message(&mut stream, Opcode::CalibreBusy, &json!({}));
                    continue;
                }
                // Then pass the connection to the real fake.
                let mut upstream = TcpStream::connect(real).unwrap();
                let mut down = stream.try_clone().unwrap();
                let mut up = upstream.try_clone().unwrap();
                std::thread::spawn(move || {
                    let _ = std::io::copy(&mut down, &mut up);
                });
                let _ = std::io::copy(&mut upstream, &mut stream);
            }
        });
        let target = Target::Address {
            host: "127.0.0.1".into(),
            port: addr.port(),
        };
        let status = check::run(&env, &target, None, &|_| {});
        assert_eq!(status.state, CheckState::Ok, "{status:?}");
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_check_connects_once_and_holds_the_address() {
        let tmp = tempfile::tempdir().unwrap();
        let env = check_env(tmp.path());
        let (addr, count) = fake_calibre("lib-9");
        let holds = Mutex::new(Vec::new());
        let by_address = Target::Address {
            host: "127.0.0.1".into(),
            port: addr.port(),
        };
        let status = check::run(&env, &by_address, None, &|a| holds.lock().unwrap().push(a));
        assert_eq!(status.state, CheckState::Ok, "{status:?}");
        let lib = status.library.unwrap();
        assert_eq!(
            (lib.uuid.as_str(), lib.name.as_str()),
            ("lib-9", "Test Library")
        );
        assert_eq!(lib.columns["#kindle_read"]["name"], "Kindle read");
        assert_eq!(*holds.lock().unwrap(), vec![Some(addr), None]);
        assert_eq!(count.load(Ordering::SeqCst), 1, "one connection");

        // A library Find has seen.
        let found = FoundLibrary {
            uuid: "lib-9".into(),
            name: "Test Library".into(),
            host: "127.0.0.1".into(),
            port: addr.port(),
            hostname: "pc".into(),
            password_required: false,
        };
        let status = check::run(&env, &Target::Found(found.clone()), None, &|_| {});
        assert_eq!(status.state, CheckState::Ok);
        assert_eq!(status.library.unwrap().hostname.as_deref(), Some("pc"));
        assert_eq!(count.load(Ordering::SeqCst), 2);

        // One that wants a password, without one: no connection at all.
        let wants = Target::Found(FoundLibrary {
            password_required: true,
            ..found
        });
        let status = check::run(&env, &wants, None, &|_| panic!("held"));
        assert_eq!(status.state, CheckState::PasswordNeeded);
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_check_reports_a_lockout_and_nothing_listening() {
        let tmp = tempfile::tempdir().unwrap();
        let env = check_env(tmp.path());
        // Calibre's lockout: it accepts, then closes at once.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || listener.incoming().for_each(drop));
        let target = Target::Address {
            host: "127.0.0.1".into(),
            port: addr.port(),
        };
        let status = check::run(&env, &target, None, &|_| {});
        assert_eq!(status.state, CheckState::LockedOut);
        assert_eq!(status.error.as_deref(), Some(check::LOCKED_OUT));

        let dead = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let target = Target::Address {
            host: "127.0.0.1".into(),
            port: dead.port(),
        };
        let status = check::run(&env, &target, None, &|_| {});
        assert_eq!(status.state, CheckState::Unreachable, "{status:?}");
    }

    #[test]
    fn a_check_of_the_connected_calibre_waits_for_the_session_to_step_aside() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path();
        // Serves one connection at a time, like Calibre serves one device.
        let (addr, count) = fake_calibre_with("lib-1", false);
        let (h, thread) = start(state, Some(("lib-1", addr)));
        wait_until("connected", || h.status().state == State::Connected);

        h.start_check(
            Target::Address {
                host: "127.0.0.1".into(),
                port: addr.port(),
            },
            None,
        );
        wait_until("the check", || {
            h.check_status()
                .is_some_and(|s| s.state != CheckState::Checking)
        });
        let s = h.check_status().unwrap();
        assert_eq!(s.state, CheckState::Ok, "{s:?}");
        assert_eq!(h.checked().unwrap().uuid, "lib-1");
        // Then the service connects again: session, check, session.
        wait_until("reconnected", || count.load(Ordering::SeqCst) >= 3);
        wait_until("connected", || h.status().state == State::Connected);
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn a_library_switch_in_calibre_ends_the_session_and_lists_the_other_library() {
        let tmp = tempfile::tempdir().unwrap();
        // Serves lib-1, switches to lib-2 during the first session, and
        // serves lib-2 until switched back.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let back = Arc::new(AtomicBool::new(false));
        let switched_back = back.clone();
        std::thread::spawn(move || {
            for (n, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else { continue };
                let back = switched_back.load(Ordering::SeqCst);
                let uuid = if n == 0 || back { "lib-1" } else { "lib-2" };
                let init = json!({"serverProtocolVersion": 1, "currentLibraryUUID": uuid,
                                  "currentLibraryName": format!("Library {uuid}"),
                                  "passwordChallenge": ""});
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                if write_message(&mut stream, Opcode::GetInitializationInfo, &init).is_err()
                    || !matches!(read_message(&mut reader), Ok(Some(_)))
                {
                    continue;
                }
                if n == 0 {
                    let other = json!({"libraryName": "Library lib-2", "libraryUuid": "lib-2",
                                       "fieldMetadata": {}});
                    let _ = write_message(&mut stream, Opcode::SetLibraryInfo, &other);
                    let _ = read_message(&mut reader);
                }
                while write_message(&mut stream, Opcode::Noop, &json!({})).is_ok()
                    && matches!(read_message(&mut reader), Ok(Some(_)))
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        });
        let (h, thread) = start(tmp.path(), Some(("lib-1", addr)));
        wait_until("the other library found", || {
            h.found().iter().any(|f| f.uuid == "lib-2")
        });
        wait_until("not connected", || h.status().state == State::NotFound);
        assert!(
            h.status()
                .detail
                .contains("serving Library lib-2, not Test Library"),
            "{}",
            h.status().detail
        );

        // Switched back in Calibre: Connect now tries that address again.
        back.store(true, Ordering::SeqCst);
        h.connect_now();
        wait_until("connected again", || h.status().state == State::Connected);
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn new_sync_settings_end_the_session_at_a_quiet_moment_and_reconnect() {
        let tmp = tempfile::tempdir().unwrap();
        let (addr, count) = fake_calibre_with("lib-1", false);
        let (h, thread) = start(tmp.path(), Some(("lib-1", addr)));
        wait_until("connected", || h.status().state == State::Connected);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        h.settings_changed();
        wait_until("reconnected", || count.load(Ordering::SeqCst) == 2);
        wait_until("connected", || h.status().state == State::Connected);
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn link_address_links_only_on_success() {
        let tmp = tempfile::tempdir().unwrap();
        let (addr, _count) = fake_calibre("lib-9");
        let port = addr.port();
        let (info, link) = link_address(
            tmp.path(),
            "127.0.0.1",
            port,
            Some("pw".into()),
            None,
            false,
        )
        .unwrap();
        assert_eq!(info.library_uuid, "lib-9");
        assert_eq!(link, Link::New);
        let config = Config::load(tmp.path()).unwrap();
        let a = config.linked("lib-9").unwrap();
        assert_eq!(a.port, Some(port));
        assert_eq!(a.password.as_deref(), Some("pw"));

        // A port with nothing listening: error, and nothing stored.
        let dead = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        assert!(matches!(
            link_address(tmp.path(), "127.0.0.1", dead.port(), None, None, true),
            Err(LinkError::Unreachable(_))
        ));
        assert!(Config::load(tmp.path()).unwrap().linked("lib-9").is_some());
    }

    #[test]
    fn link_address_checks_the_uuid_and_asks_before_replacing() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, _) = fake_calibre("lib-a");
        let (b, _) = fake_calibre("lib-b");
        link_address(
            tmp.path(),
            "127.0.0.1",
            a.port(),
            None,
            Some("lib-a"),
            false,
        )
        .unwrap();

        let r = link_address(tmp.path(), "127.0.0.1", b.port(), None, Some("lib-x"), true);
        assert!(matches!(r, Err(LinkError::OtherLibrary(i)) if i.library_uuid == "lib-b"));

        let r = link_address(
            tmp.path(),
            "127.0.0.1",
            b.port(),
            None,
            Some("lib-b"),
            false,
        );
        assert!(
            matches!(r, Err(LinkError::NeedsReplace { linked, answered })
            if linked.uuid == "lib-a" && answered.library_uuid == "lib-b")
        );
        assert!(Config::load(tmp.path()).unwrap().linked("lib-a").is_some());

        let (_, link) =
            link_address(tmp.path(), "127.0.0.1", b.port(), None, Some("lib-b"), true).unwrap();
        assert!(matches!(link, Link::Replaced(old) if old.uuid == "lib-a"));
        assert!(Config::load(tmp.path()).unwrap().linked("lib-b").is_some());
    }

    #[test]
    fn find_reuses_what_a_server_answered_until_wifi_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let (addr, count) = fake_calibre("lib-new");
        let server = CalibreServer {
            addr: addr.ip(),
            hostname: "pc".into(),
            port: addr.port(),
            content_server_port: None,
        };
        let config = Config::load(tmp.path()).unwrap();
        let (h, thread) = start(tmp.path(), None);
        let cached = |a| h.cached_find(a);
        let search = || find_libraries(&config, std::slice::from_ref(&server), &cached);

        let results = search();
        let FindResult::Answered(info) = &results[0].1 else {
            panic!("expected a fresh answer");
        };
        h.remember_find(addr, info.clone());
        assert!(matches!(&search()[0].1, FindResult::Cached(i) if i.library_uuid == "lib-new"));
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "one connection for two searches"
        );

        h.network_changed(false);
        h.network_changed(true);
        assert!(matches!(&search()[0].1, FindResult::Answered(_)));
        assert_eq!(count.load(Ordering::SeqCst), 2);
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn while_retrying_the_status_says_when_and_why() {
        let tmp = tempfile::tempdir().unwrap();
        let dead = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let (h, thread) =
            start_with_backoff(tmp.path(), Some(("lib-1", dead)), Duration::from_secs(60));
        wait_until("not found", || h.status().state == State::NotFound);
        let s = h.status();
        let left = s.retry_at.unwrap() - unix_now();
        assert!((58..=60).contains(&left), "{left}");
        assert_eq!(s.advice, None, "nothing to add to \"not available\"");
        h.shutdown();
        thread.join().unwrap();

        // A lockout: the advice says how to clear it.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || listener.incoming().for_each(drop));
        let (h, thread) =
            start_with_backoff(tmp.path(), Some(("lib-1", addr)), Duration::from_secs(60));
        wait_until("the lockout", || h.status().state == State::LockedOut);
        assert_eq!(h.status().advice.as_deref(), Some(check::LOCKED_OUT));
        assert!(h.status().retry_at.is_some());
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn a_calibre_that_closes_at_once_is_reported_as_a_lockout() {
        let tmp = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || listener.incoming().for_each(drop));
        let (h, thread) = start(tmp.path(), Some(("lib-1", addr)));
        wait_until("the lockout", || h.status().state == State::LockedOut);
        assert!(h.status().detail.starts_with(check::LOCKED_OUT));
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn find_libraries_skips_the_linked_server() {
        let tmp = tempfile::tempdir().unwrap();
        let (addr, count) = fake_calibre("lib-new");
        let server = CalibreServer {
            addr: addr.ip(),
            hostname: "pc".into(),
            port: addr.port(),
            content_server_port: None,
        };
        let config = Config::load(tmp.path()).unwrap();
        let results = find_libraries(&config, std::slice::from_ref(&server), &|_| None);
        assert!(matches!(&results[0].1, FindResult::Answered(i) if i.library_uuid == "lib-new"));
        assert!(
            !tmp.path().join(config::CONFIG_FILE).exists(),
            "find stores nothing"
        );

        // Once linked, find leaves that server alone.
        let found = FoundLibrary::new(
            &CalibreInfo {
                library_uuid: "lib-new".into(),
                ..Default::default()
            },
            addr.ip().to_string(),
            addr.port(),
            "pc".into(),
        );
        assert_eq!(link_found(tmp.path(), &found, false).unwrap(), Link::New);
        let config = Config::load(tmp.path()).unwrap();
        let results = find_libraries(&config, &[server], &|_| None);
        assert!(matches!(results[0].1, FindResult::Linked));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn candidates_prefer_discovered_matches_then_the_last_address() {
        let a = LinkedLibrary {
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
        let got = candidates(&a, &found, &HashMap::new());
        let addrs: Vec<String> = got.iter().map(|c| c.addr.to_string()).collect();
        assert_eq!(addrs, vec!["10.0.0.7:4242", "10.0.0.2:9090"]);
        let skip: HashMap<SocketAddr, String> =
            [("10.0.0.7:4242".parse().unwrap(), "Other".into())].into();
        assert_eq!(candidates(&a, &found, &skip).len(), 1);
    }
}
