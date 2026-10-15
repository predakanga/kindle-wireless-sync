//! `kws`: command-line front end, used for host testing in Phase 2.

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use kws::discovery::{self, CalibreServer};
use kws::library::Library;
use kws::protocol::Connection;
use kws::session::{self, DeviceConfig, SessionEnd};
use lexopt::ValueExt;

const USAGE: &str = "\
usage:
  kws discover [--timeout SECS] [--probe ADDR]...
  kws connect --books DIR --state DIR [--host HOST] [--port PORT]
              [--password PW] [--allow LIBRARY_UUID]... [--allow-any]
              [--name NAME] [--transcript FILE]

Set RUST_LOG=debug to log every protocol message.";

/// Calibre pings an idle device every few seconds; silence this long means
/// the connection is dead.
const READ_TIMEOUT: Duration = Duration::from_secs(300);

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

fn real_main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let mut args = lexopt::Parser::from_env();
    let cmd = match args.next()? {
        Some(lexopt::Arg::Value(v)) => v.string()?,
        Some(lexopt::Arg::Long("help")) | Some(lexopt::Arg::Short('h')) | None => {
            println!("{USAGE}");
            return Ok(ExitCode::SUCCESS);
        }
        Some(other) => return Err(other.unexpected().into()),
    };
    match cmd.as_str() {
        "discover" => discover_cmd(args),
        "connect" => connect_cmd(args),
        _ => Err(format!("unknown command {cmd:?}\n{USAGE}").into()),
    }
}

fn discover_cmd(mut args: lexopt::Parser) -> Result<ExitCode, Box<dyn std::error::Error>> {
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

fn connect_cmd(mut args: lexopt::Parser) -> Result<ExitCode, Box<dyn std::error::Error>> {
    use lexopt::prelude::*;
    let mut books: Option<PathBuf> = None;
    let mut state: Option<PathBuf> = None;
    let mut host: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut transcript: Option<PathBuf> = None;
    let mut allowed: Vec<String> = Vec::new();
    let mut allow_any = false;
    let mut cfg = DeviceConfig::default();
    while let Some(arg) = args.next()? {
        match arg {
            Long("books") => books = Some(args.value()?.into()),
            Long("state") => state = Some(args.value()?.into()),
            Long("host") => host = Some(args.value()?.string()?),
            Long("port") => port = Some(args.value()?.parse()?),
            Long("password") => cfg.password = Some(args.value()?.string()?),
            Long("allow") => allowed.push(args.value()?.string()?),
            Long("allow-any") => allow_any = true,
            Long("name") => cfg.device_name = args.value()?.string()?,
            Long("transcript") => transcript = Some(args.value()?.into()),
            _ => return Err(arg.unexpected().into()),
        }
    }
    let (books, state) = match (books, state) {
        (Some(b), Some(s)) => (b, s),
        _ => return Err(format!("--books and --state are required\n{USAGE}").into()),
    };

    let addr = resolve_server(host, port)?;
    log::info!("connecting to {addr}");
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10))?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_nodelay(true)?;
    let mut conn = Connection::new(BufReader::new(stream.try_clone()?), BufWriter::new(stream));
    if let Some(path) = transcript {
        conn = conn.with_transcript(Box::new(File::create(path)?));
    }

    let mut lib = Library::open(&books, &state)?;
    let end = session::run(&mut conn, &mut lib, &cfg, &mut |info| {
        allow_any || allowed.contains(&info.library_uuid)
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
    let found = discovery::discover(Duration::from_secs(3), &targets)?;
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
