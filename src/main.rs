// SPDX-License-Identifier: GPL-3.0-or-later
//! `kindle-wireless-sync`: the daemon and its command-line tools.

use std::net::{IpAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use kws::api::{self, Api};
use kws::catalog::Catalog;
use kws::config::{self, Config, Link};
use kws::daemon::{self, Daemon, FindResult, LinkError, Options};
use kws::discovery;
use kws::kindle::{self, CoverConfig};
use kws::library::Library;
use kws::network;
use kws::readsync::{Columns, DEFAULT_LAST_READ_COLUMN, DEFAULT_READ_COLUMN};
use kws::session::DeviceConfig;
use lexopt::ValueExt;

const USAGE: &str = "\
usage:
  kindle-wireless-sync daemon --books DIR --state DIR [--thumbnails DIR [--catalog DB]] [--name NAME]
              [--api-port PORT]
  kindle-wireless-sync find --state DIR [--probe ADDR]...
  kindle-wireless-sync link --state DIR [HOST [PORT]] [--uuid UUID] [--replace]
              [--password PW] [--writable | --read-only]
  kindle-wireless-sync unlink --state DIR
  kindle-wireless-sync status --state DIR
  kindle-wireless-sync series --books DIR --catalog DB --remove-all

daemon      the service: connects to the linked library (ADR 0008, 0018)
find        \"Find libraries\": broadcast, then connect once to each server to
            read its library, and print what answered
link        link this Kindle to the library at HOST:PORT (port 9090 if not
            given), which must answer now. --uuid refuses any other library.
            --replace confirms replacing a different linked library: the
            books stay, but each book's metadata from that library goes.
            --writable lets this Kindle send read status and last-read date
            to the library (ADR 0017), --read-only stops it. Without HOST,
            changes the linked library's --password or --writable setting
unlink      stop syncing with the linked library; books stay
status      show the daemon status and the linked library
series      --remove-all: dissolve the series this app created (uninstall)

Set RUST_LOG=debug to log every protocol message.";

#[cfg(feature = "dev-tools")]
const DEV_USAGE: &str = "
Development commands (dev-tools feature):
  kindle-wireless-sync discover [--timeout SECS] [--probe ADDR]...
  kindle-wireless-sync connect --books DIR --state DIR [--host HOST] [--port PORT]
              [--password PW] [--allow LIBRARY_UUID]... [--allow-any]
              [--name NAME] [--transcript FILE] [--thumbnails DIR [--catalog DB]]

  kindle-wireless-sync check --state DIR HOST [PORT] [--password PW] [--quiet SECS]

discover    broadcast only; lists servers without connecting
connect     run a single session in the foreground
check       check a library as the app does before linking it (ADR 0020):
            password, columns, and nothing synced. --quiet is how long
            Calibre must be quiet before the check leaves (default 3)";

/// Calibre pings an idle device every few seconds; silence this long means
/// the connection is dead.
const READ_TIMEOUT: Duration = Duration::from_secs(300);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);
/// Calibre's default port for wireless device connections.
const CALIBRE_PORT: u16 = 9090;

type CmdResult = Result<ExitCode, Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    match real_main() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn real_main() -> CmdResult {
    let mut args = lexopt::Parser::from_env();
    let cmd = match args.next()? {
        Some(lexopt::Arg::Value(v)) => v.string()?,
        Some(lexopt::Arg::Long("help")) | Some(lexopt::Arg::Short('h')) | None => {
            print_usage();
            return Ok(ExitCode::SUCCESS);
        }
        Some(other) => return Err(other.unexpected().into()),
    };
    match cmd.as_str() {
        "daemon" => daemon_cmd(args),
        "find" => find_cmd(args),
        "link" => link_cmd(args),
        "unlink" => unlink_cmd(args),
        "status" => status_cmd(args),
        "series" => series_cmd(args),
        #[cfg(feature = "dev-tools")]
        "discover" => dev::discover_cmd(args),
        #[cfg(feature = "dev-tools")]
        "connect" => dev::connect_cmd(args),
        #[cfg(feature = "dev-tools")]
        "check" => dev::check_cmd(args),
        _ => Err(format!("unknown command {cmd:?}\n{USAGE}").into()),
    }
}

fn print_usage() {
    println!("{USAGE}");
    #[cfg(feature = "dev-tools")]
    println!("{DEV_USAGE}");
}

fn required<T>(v: Option<T>, flag: &str) -> Result<T, Box<dyn std::error::Error>> {
    v.ok_or_else(|| format!("{flag} is required\n{USAGE}").into())
}

fn open_library(
    books: PathBuf,
    state: &Path,
    thumbnails: Option<PathBuf>,
    catalog: Option<PathBuf>,
) -> std::io::Result<Library> {
    let mut lib = Library::open(&books, state)?;
    if let Some(thumbnails_dir) = thumbnails {
        lib.enable_covers(CoverConfig {
            thumbnails_dir,
            catalog_db: catalog,
        })?;
    }
    Ok(lib)
}

fn daemon_cmd(mut args: lexopt::Parser) -> CmdResult {
    use lexopt::prelude::*;
    let (mut books, mut state, mut thumbnails, mut catalog) = (None, None, None, None);
    let mut api_port = api::DEFAULT_PORT;
    let device = DeviceConfig::default();
    let mut name: Option<String> = None;
    while let Some(arg) = args.next()? {
        match arg {
            Long("books") => books = Some(args.value()?.into()),
            Long("state") => state = Some(args.value()?.into()),
            Long("thumbnails") => thumbnails = Some(args.value()?.into()),
            Long("catalog") => catalog = Some(args.value()?.into()),
            Long("name") => name = Some(args.value()?.string()?),
            Long("api-port") => api_port = args.value()?.parse()?,
            _ => return Err(arg.unexpected().into()),
        }
    }
    let state: PathBuf = required(state, "--state")?;
    // The autostart sentinel and guard files sit next to the state
    // directory (ADR 0010).
    let home = state
        .canonicalize()
        .ok()
        .and_then(|s| s.parent().map(Path::to_path_buf))
        .ok_or("--state must be an existing directory with a parent")?;
    let mut lib = open_library(
        required(books, "--books")?,
        &state,
        thumbnails,
        catalog.clone(),
    )?;
    // Series (ADR 0016) need the Kindle's catalog and its change endpoint.
    // Passes wait until the catalog is ready, which at boot can be after we
    // start.
    if let Some(db) = catalog.filter(|db| db.is_file()) {
        // Read-status sync (ADR 0017) only reads it.
        lib.set_catalog(Catalog::new(db.clone()));
        let debounce = kws::library::SERIES_DEBOUNCE;
        if Config::load(&state).map(|c| c.series).unwrap_or(true) {
            lib.enable_series(Catalog::new(db), debounce);
        } else {
            lib.dissolve_series(Catalog::new(db), debounce);
        }
    }

    // On a Kindle, follow Wi-Fi over lipc; elsewhere assume the network is up.
    let wifi = network::wifi_connected();
    let opts = Options {
        device,
        state_dir: state.clone(),
        backoff: Options::default_backoff(),
        read_timeout: READ_TIMEOUT,
        discover: Box::new(|| {
            discovery::discover(DISCOVERY_TIMEOUT, &discovery::default_targets()).unwrap_or_else(
                |e| {
                    log::warn!("discovery failed: {e}");
                    Vec::new()
                },
            )
        }),
        // --name wins; otherwise the name set on the Kindle (ADR 0015).
        device_name: Box::new(move || {
            name.clone()
                .or_else(kindle::device_name)
                .unwrap_or_else(|| DeviceConfig::default().device_name)
        }),
        quiet: kws::session::QUIET_BEFORE_RECONNECT,
    };
    let (daemon, handle) = Daemon::new(lib, opts, wifi.unwrap_or(true));
    if wifi.is_some() {
        let h = handle.clone();
        network::watch_wifi(move |up| h.network_changed(up));
    } else {
        log::info!("lipc not available; assuming the network is always up");
    }
    // The UI's API (ADR 0011). Without it the daemon still syncs.
    match TcpListener::bind(("127.0.0.1", api_port)) {
        Ok(listener) => {
            let api = Api {
                handle: handle.clone(),
                state_dir: state.clone(),
                home_dir: home,
                discover: Box::new(|| {
                    discovery::discover(DISCOVERY_TIMEOUT, &discovery::default_targets())
                        .unwrap_or_default()
                }),
            };
            std::thread::spawn(move || api::serve(listener, api));
            log::info!("api listening on 127.0.0.1:{api_port}");
        }
        Err(e) => log::error!("cannot listen on 127.0.0.1:{api_port}: {e}; the UI won't work"),
    }
    daemon.run();
    Ok(ExitCode::SUCCESS)
}

fn find_cmd(mut args: lexopt::Parser) -> CmdResult {
    use lexopt::prelude::*;
    let mut state: Option<PathBuf> = None;
    let mut targets: Vec<IpAddr> = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Long("state") => state = Some(args.value()?.into()),
            Long("probe") => targets.push(args.value()?.parse()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let state = required(state, "--state")?;
    if targets.is_empty() {
        targets = discovery::default_targets();
    }
    let found = discovery::discover(DISCOVERY_TIMEOUT, &targets)?;
    if found.is_empty() {
        println!("no calibre found (is \"Start wireless device connection\" on?)");
        return Ok(ExitCode::FAILURE);
    }
    let config = Config::load(&state)?;
    for (server, result) in daemon::find_libraries(&config, &found) {
        let at = format!("{} at {}:{}", server.hostname, server.addr, server.port);
        match result {
            FindResult::Linked => println!("{at}: the linked library's server; not contacted"),
            FindResult::Answered(info) if config.linked(&info.library_uuid).is_some() => {
                println!("{at}: library {:?} (linked)", info.library_name)
            }
            FindResult::Answered(info) => println!(
                "{at}: library {:?} ({})\n  to link it: link --state {} {} {} --uuid {}",
                info.library_name,
                info.library_uuid,
                state.display(),
                server.addr,
                server.port,
                info.library_uuid
            ),
            FindResult::Failed(e) => println!("{at}: {e}"),
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn link_cmd(mut args: lexopt::Parser) -> CmdResult {
    use lexopt::prelude::*;
    let (mut state, mut uuid, mut password): (Option<PathBuf>, Option<String>, Option<String>) =
        (None, None, None);
    let mut writable: Option<bool> = None;
    let mut replace = false;
    let mut positional: Vec<String> = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Long("state") => state = Some(args.value()?.into()),
            Long("uuid") => uuid = Some(args.value()?.string()?),
            Long("password") => password = Some(args.value()?.string()?),
            Long("writable") if writable.is_none() => writable = Some(true),
            Long("read-only") if writable.is_none() => writable = Some(false),
            Long("replace") => replace = true,
            Value(v) if positional.len() < 2 => positional.push(v.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let state = required(state, "--state")?;
    let linked_uuid = match positional.as_slice() {
        [] => {
            if uuid.is_some() || replace {
                return Err(format!("--uuid and --replace need HOST\n{USAGE}").into());
            }
            if password.is_none() && writable.is_none() {
                return Err(format!("link needs HOST, or a setting to change\n{USAGE}").into());
            }
            let Some(lib) = Config::load(&state)?.library else {
                println!("no library is linked; give its HOST");
                return Ok(ExitCode::FAILURE);
            };
            if let Some(pw) = password {
                let pw = (!pw.is_empty()).then_some(pw);
                let set = pw.is_some();
                config::update(&state, |c| c.set_password(&lib.uuid, pw))?;
                println!(
                    "{}",
                    if set {
                        "password set"
                    } else {
                        "password cleared"
                    }
                );
            }
            lib.uuid
        }
        [host, port @ ..] => {
            let port = match port.first() {
                Some(p) => p.parse()?,
                None => CALIBRE_PORT,
            };
            let password = password.filter(|p| !p.is_empty());
            match daemon::link_address(&state, host, port, password, uuid.as_deref(), replace) {
                Ok((info, link)) => {
                    let lib = format!("{:?} ({})", info.library_name, info.library_uuid);
                    match link {
                        Link::Updated => {
                            println!("updated the linked library {lib}: {host}:{port}")
                        }
                        Link::Replaced(old) => {
                            println!(
                                "linked library {lib} at {host}:{port}, replacing {:?} ({})",
                                old.name, old.uuid
                            );
                            end_session();
                        }
                        _ => println!("linked library {lib} at {host}:{port}"),
                    }
                    info.library_uuid
                }
                Err(LinkError::NeedsReplace { linked, answered }) => {
                    println!(
                        "this Kindle is linked to {:?} ({}). Linking {:?} ({}) replaces it: \
                         the books stay, but each book's metadata from {:?} goes. \
                         Rerun with --replace to do that.",
                        linked.name,
                        linked.uuid,
                        answered.library_name,
                        answered.library_uuid,
                        linked.name
                    );
                    return Ok(ExitCode::FAILURE);
                }
                Err(LinkError::OtherLibrary(info)) => {
                    println!(
                        "{host}:{port} serves library {:?} ({}), not {}; nothing changed",
                        info.library_name,
                        info.library_uuid,
                        uuid.unwrap_or_default()
                    );
                    return Ok(ExitCode::FAILURE);
                }
                Err(LinkError::Unreachable(e)) => {
                    println!("could not reach Calibre at {host}:{port}: {e}");
                    return Ok(ExitCode::FAILURE);
                }
                Err(LinkError::Settings(e)) => return Err(e.into()),
            }
        }
    };
    if let Some(w) = writable {
        // The app will let users pick the columns; for now every caller
        // stores the fixed names (ADR 0017).
        config::update(&state, |c| {
            c.set_writable(&linked_uuid, w.then(Columns::defaults))
        })?;
        if w {
            println!(
                "writable: read status and last-read date go to the library's \
                 {DEFAULT_READ_COLUMN} and {DEFAULT_LAST_READ_COLUMN} columns, once you create them"
            );
        } else {
            println!("read-only: this Kindle won't change the library");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// After the link changed: the running service may still be in a session
/// with the old library, so ask it to end that session now (ADR 0018).
fn end_session() {
    match api::request_reconnect(api::DEFAULT_PORT) {
        Ok(true) => println!("the service ended its session and is reconnecting"),
        Ok(false) => {}
        Err(e) => println!(
            "could not reach the service ({e}); it will use the new link from its next connection"
        ),
    }
}

fn unlink_cmd(mut args: lexopt::Parser) -> CmdResult {
    use lexopt::prelude::*;
    let mut state: Option<PathBuf> = None;
    while let Some(arg) = args.next()? {
        match arg {
            Long("state") => state = Some(args.value()?.into()),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let state = required(state, "--state")?;
    if config::update(&state, |c| c.unlink())? {
        println!("unlinked; the books stay on the Kindle");
        end_session();
        Ok(ExitCode::SUCCESS)
    } else {
        println!("no library was linked");
        Ok(ExitCode::FAILURE)
    }
}

/// `series --remove-all`: dissolve our series (ADR 0016), for uninstall.
fn series_cmd(mut args: lexopt::Parser) -> CmdResult {
    use lexopt::prelude::*;
    let (mut books, mut catalog, mut remove_all) = (None::<PathBuf>, None::<PathBuf>, false);
    while let Some(arg) = args.next()? {
        match arg {
            Long("books") => books = Some(args.value()?.into()),
            Long("catalog") => catalog = Some(args.value()?.into()),
            Long("remove-all") => remove_all = true,
            _ => return Err(arg.unexpected().into()),
        }
    }
    if !remove_all {
        return Err(format!("series needs --remove-all\n{USAGE}").into());
    }
    let books = required(books, "--books")?.canonicalize()?;
    let n =
        kws::library::remove_all_series(&Catalog::new(required(catalog, "--catalog")?), &books)?;
    println!("removed our series ({n} catalog changes)");
    Ok(ExitCode::SUCCESS)
}

fn status_cmd(mut args: lexopt::Parser) -> CmdResult {
    use lexopt::prelude::*;
    let mut state: Option<PathBuf> = None;
    while let Some(arg) = args.next()? {
        match arg {
            Long("state") => state = Some(args.value()?.into()),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let state = required(state, "--state")?;
    match std::fs::read_to_string(state.join(daemon::STATUS_FILE)) {
        Ok(s) => println!("{s}"),
        Err(_) => println!("no status yet (daemon not run)"),
    }
    match Config::load(&state)?.library {
        Some(a) => println!(
            "linked: {:?} ({}) last at {:?}:{:?}, {}",
            a.name,
            a.uuid,
            a.host,
            a.port,
            if a.writable { "writable" } else { "read-only" }
        ),
        None => println!("no library linked"),
    }
    Ok(ExitCode::SUCCESS)
}

/// Development commands, compiled only with the `dev-tools` feature so the
/// packaged binary stays small and its help lists only user commands.
#[cfg(feature = "dev-tools")]
mod dev {
    use std::fs::File;
    use std::io::{BufReader, BufWriter};
    use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
    use std::path::PathBuf;
    use std::process::ExitCode;
    use std::time::Duration;

    use kws::discovery::{self, CalibreServer};
    use kws::protocol::Connection;
    use kws::session::{self, Admission, CalibreInfo, DeviceConfig, SessionEnd};

    use super::{open_library, required, CmdResult, DISCOVERY_TIMEOUT, READ_TIMEOUT};

    pub fn discover_cmd(mut args: lexopt::Parser) -> CmdResult {
        use lexopt::prelude::*;
        let mut timeout = 3u64;
        let mut targets: Vec<IpAddr> = Vec::new();
        while let Some(arg) = args.next()? {
            match arg {
                Long("timeout") => timeout = args.value()?.parse()?,
                Long("probe") => targets.push(args.value()?.parse()?),
                _ => return Err(arg.unexpected().into()),
            }
        }
        if targets.is_empty() {
            targets = discovery::default_targets();
        }
        let found = discovery::discover(Duration::from_secs(timeout), &targets)?;
        if found.is_empty() {
            println!("no calibre found (is \"Start wireless device connection\" on?)");
            return Ok(ExitCode::FAILURE);
        }
        for s in found {
            println!("{} at {}:{}", s.hostname, s.addr, s.port);
        }
        Ok(ExitCode::SUCCESS)
    }

    pub fn check_cmd(mut args: lexopt::Parser) -> CmdResult {
        use kws::check::{self, CheckState, Target};
        use lexopt::prelude::*;
        let mut state: Option<PathBuf> = None;
        let mut host: Option<String> = None;
        let mut port = super::CALIBRE_PORT;
        let mut password: Option<String> = None;
        let mut quiet = kws::check::QUIET;
        while let Some(arg) = args.next()? {
            match arg {
                Long("state") => state = Some(args.value()?.into()),
                Long("password") => password = Some(args.value()?.string()?),
                Long("quiet") => quiet = Duration::from_secs_f64(args.value()?.parse()?),
                Value(v) if host.is_none() => host = Some(v.string()?),
                Value(v) => port = v.parse()?,
                _ => return Err(arg.unexpected().into()),
            }
        }
        let state: PathBuf = required(state, "--state")?;
        std::fs::create_dir_all(state.join("books"))?;
        let env = check::Env {
            books_dir: state.join("books"),
            state_dir: state,
            device: DeviceConfig::default(),
            quiet,
        };
        let target = Target::Address {
            host: required(host, "HOST")?,
            port,
        };
        let started = std::time::Instant::now();
        let status = check::run(&env, &target, password, &|_| {});
        println!(
            "{} after {:.1}s: {}",
            status.state.as_str(),
            started.elapsed().as_secs_f64(),
            serde_json::to_string_pretty(&status.to_json())?
        );
        Ok(if status.state == CheckState::Ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        })
    }

    pub fn connect_cmd(mut args: lexopt::Parser) -> CmdResult {
        use lexopt::prelude::*;
        let mut books: Option<PathBuf> = None;
        let mut state: Option<PathBuf> = None;
        let mut host: Option<String> = None;
        let mut port: Option<u16> = None;
        let mut transcript: Option<PathBuf> = None;
        let mut thumbnails: Option<PathBuf> = None;
        let mut catalog: Option<PathBuf> = None;
        let mut password: Option<String> = None;
        let mut allowed: Vec<String> = Vec::new();
        let mut allow_any = false;
        let mut cfg = DeviceConfig::default();
        while let Some(arg) = args.next()? {
            match arg {
                Long("books") => books = Some(args.value()?.into()),
                Long("state") => state = Some(args.value()?.into()),
                Long("host") => host = Some(args.value()?.string()?),
                Long("port") => port = Some(args.value()?.parse()?),
                Long("password") => password = Some(args.value()?.string()?),
                Long("allow") => allowed.push(args.value()?.string()?),
                Long("allow-any") => allow_any = true,
                Long("name") => cfg.device_name = args.value()?.string()?,
                Long("transcript") => transcript = Some(args.value()?.into()),
                Long("thumbnails") => thumbnails = Some(args.value()?.into()),
                Long("catalog") => catalog = Some(args.value()?.into()),
                _ => return Err(arg.unexpected().into()),
            }
        }
        let state: PathBuf = required(state, "--state")?;
        let mut lib = open_library(required(books, "--books")?, &state, thumbnails, catalog)?;

        let addr = resolve_server(host, port)?;
        log::info!("connecting to {addr}");
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10))?;
        stream.set_read_timeout(Some(READ_TIMEOUT))?;
        stream.set_nodelay(true)?;
        let mut conn = Connection::new(BufReader::new(stream.try_clone()?), BufWriter::new(stream));
        if let Some(path) = transcript {
            conn = conn.with_transcript(Box::new(File::create(path)?));
        }

        let end = session::run(&mut conn, &mut lib, &cfg, &mut |info: &CalibreInfo| {
            if allow_any || allowed.contains(&info.library_uuid) {
                Admission::Accept {
                    password: password.clone(),
                    columns: None,
                    known_columns: Default::default(),
                }
            } else {
                Admission::Reject
            }
        })?;
        match end {
            SessionEnd::Ejected | SessionEnd::Closed => {
                println!("disconnected ({end:?})");
                Ok(ExitCode::SUCCESS)
            }
            SessionEnd::LibraryRejected(info) => {
                println!(
                    "library {:?} is not allowed; rerun with --allow {}",
                    info.library_name, info.library_uuid
                );
                Ok(ExitCode::FAILURE)
            }
            other => {
                println!("session ended: {other:?}");
                Ok(ExitCode::FAILURE)
            }
        }
    }

    /// Picks the Calibre server: explicit host and port, a unicast probe of the
    /// host, or a broadcast that must find exactly one server.
    fn resolve_server(
        host: Option<String>,
        port: Option<u16>,
    ) -> Result<SocketAddr, Box<dyn std::error::Error>> {
        if let (Some(h), Some(p)) = (&host, port) {
            return (h.as_str(), p)
                .to_socket_addrs()?
                .next()
                .ok_or_else(|| "cannot resolve host".into());
        }
        let targets = match &host {
            Some(h) => (h.as_str(), 0).to_socket_addrs()?.map(|a| a.ip()).collect(),
            None => discovery::default_targets(),
        };
        let found = discovery::discover(DISCOVERY_TIMEOUT, &targets)?;
        match found.as_slice() {
            [] => Err("no calibre found (is \"Start wireless device connection\" on?)".into()),
            [CalibreServer { addr, port: p, .. }] => Ok(SocketAddr::new(*addr, port.unwrap_or(*p))),
            many => {
                let list: Vec<String> = many
                    .iter()
                    .map(|s| format!("{} at {}:{}", s.hostname, s.addr, s.port))
                    .collect();
                Err(format!(
                    "several calibre servers found; pick one with --host:\n  {}",
                    list.join("\n  ")
                )
                .into())
            }
        }
    }
}
