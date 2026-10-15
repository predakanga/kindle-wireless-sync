//! `kindle-wireless-sync`: the daemon and its command-line tools.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use kws::config::{self, Config};
use kws::daemon::{self, Daemon, FindResult, Options};
use kws::discovery;
use kws::kindle::CoverConfig;
use kws::library::Library;
use kws::network;
use kws::session::DeviceConfig;
use lexopt::ValueExt;

const USAGE: &str = "\
usage:
  kindle-wireless-sync daemon --books DIR --state DIR [--thumbnails DIR [--catalog DB]] [--name NAME]
  kindle-wireless-sync find --state DIR [--probe ADDR]...
  kindle-wireless-sync add-server --state DIR HOST PORT [--password PW]
  kindle-wireless-sync allow --state DIR LIBRARY_UUID
  kindle-wireless-sync status --state DIR

daemon      the service: connects to allowed libraries (ADR 0008)
find        \"Find libraries\": broadcast, then connect once to each server to
            read its library, and list new ones as seen
add-server  connect to HOST:PORT now and allow its library if it answers
allow       allow a library listed by find
status      show the daemon status and the allowed and seen libraries

Set RUST_LOG=debug to log every protocol message.";

#[cfg(feature = "dev-tools")]
const DEV_USAGE: &str = "
Development commands (dev-tools feature):
  kindle-wireless-sync discover [--timeout SECS] [--probe ADDR]...
  kindle-wireless-sync connect --books DIR --state DIR [--host HOST] [--port PORT]
              [--password PW] [--allow LIBRARY_UUID]... [--allow-any]
              [--name NAME] [--transcript FILE] [--thumbnails DIR [--catalog DB]]

discover    broadcast only; lists servers without connecting
connect     run a single session in the foreground";

/// Calibre pings an idle device every few seconds; silence this long means
/// the connection is dead.
const READ_TIMEOUT: Duration = Duration::from_secs(300);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);

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
        "add-server" => add_server_cmd(args),
        "allow" => allow_cmd(args),
        "status" => status_cmd(args),
        #[cfg(feature = "dev-tools")]
        "discover" => dev::discover_cmd(args),
        #[cfg(feature = "dev-tools")]
        "connect" => dev::connect_cmd(args),
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
    let mut device = DeviceConfig::default();
    while let Some(arg) = args.next()? {
        match arg {
            Long("books") => books = Some(args.value()?.into()),
            Long("state") => state = Some(args.value()?.into()),
            Long("thumbnails") => thumbnails = Some(args.value()?.into()),
            Long("catalog") => catalog = Some(args.value()?.into()),
            Long("name") => device.device_name = args.value()?.string()?,
            _ => return Err(arg.unexpected().into()),
        }
    }
    let state: PathBuf = required(state, "--state")?;
    let lib = open_library(required(books, "--books")?, &state, thumbnails, catalog)?;

    // On a Kindle, follow Wi-Fi over lipc; elsewhere assume the network is up.
    let wifi = network::wifi_connected();
    let opts = Options {
        device,
        state_dir: state,
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
    };
    let (daemon, handle) = Daemon::new(lib, opts, wifi.unwrap_or(true));
    if wifi.is_some() {
        let h = handle.clone();
        network::watch_wifi(move |up| h.network_changed(up));
    } else {
        log::info!("lipc not available; assuming the network is always up");
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
    for (server, result) in daemon::find_libraries(&state, &found)? {
        let at = format!("{} at {}:{}", server.hostname, server.addr, server.port);
        match result {
            FindResult::Allowed => println!("{at}: an allowed library's server; not contacted"),
            FindResult::Answered(info) => println!(
                "{at}: library {:?} ({})",
                info.library_name, info.library_uuid
            ),
            FindResult::Failed(e) => println!("{at}: {e}"),
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn add_server_cmd(mut args: lexopt::Parser) -> CmdResult {
    use lexopt::prelude::*;
    let (mut state, mut password): (Option<PathBuf>, Option<String>) = (None, None);
    let mut positional: Vec<String> = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Long("state") => state = Some(args.value()?.into()),
            Long("password") => password = Some(args.value()?.string()?),
            Value(v) => positional.push(v.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let [host, port] = positional.as_slice() else {
        return Err(format!("add-server needs HOST and PORT\n{USAGE}").into());
    };
    let state = required(state, "--state")?;
    match daemon::add_server(&state, host, port.parse()?, password) {
        Ok(info) => {
            println!(
                "added library {:?} ({}) at {host}:{port}",
                info.library_name, info.library_uuid
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            println!("could not add {host}:{port}: {e}");
            Ok(ExitCode::FAILURE)
        }
    }
}

fn allow_cmd(mut args: lexopt::Parser) -> CmdResult {
    use lexopt::prelude::*;
    let (mut state, mut uuid): (Option<PathBuf>, Option<String>) = (None, None);
    while let Some(arg) = args.next()? {
        match arg {
            Long("state") => state = Some(args.value()?.into()),
            Value(v) if uuid.is_none() => uuid = Some(v.string()?),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let uuid = required(uuid, "LIBRARY_UUID")?;
    let state = required(state, "--state")?;
    let (already, allowed) = config::update(&state, |c| {
        (c.allowed(&uuid).is_some(), c.allow_seen(&uuid))
    })?;
    if already {
        println!("already allowed");
    } else if allowed {
        println!("allowed");
    } else {
        println!("unknown library; run `find` first, or use `add-server`");
        return Ok(ExitCode::FAILURE);
    }
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
    let config = Config::load(&state)?;
    for a in &config.allowed {
        println!(
            "allowed: {:?} ({}) last at {:?}:{:?}",
            a.name, a.uuid, a.host, a.port
        );
    }
    for s in &config.seen {
        println!("seen: {:?} ({}) at {}:{}", s.name, s.uuid, s.host, s.port);
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
    use kws::session::{self, Admission, DeviceConfig, SessionEnd};

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

        let end = session::run(&mut conn, &mut lib, &cfg, &mut |info| {
            if allow_any || allowed.contains(&info.library_uuid) {
                Admission::Accept {
                    password: password.clone(),
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
