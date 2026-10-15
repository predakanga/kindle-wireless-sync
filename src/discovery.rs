//! Finding Calibre servers by UDP broadcast.
//!
//! Calibre listens on the first free port of [`BROADCAST_PORTS`] and answers
//! any datagram with `calibre wireless device client (on <host>);<content
//! server port>,<smart device port>`.

use std::io;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::time::{Duration, Instant};

pub const BROADCAST_PORTS: [u16; 5] = [54982, 48123, 39001, 44044, 59678];
const REPLY_PREFIX: &str = "calibre wireless device client (on ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalibreServer {
    pub addr: IpAddr,
    pub hostname: String,
    /// Port of Calibre's smart device listener, the one we connect to.
    pub port: u16,
    pub content_server_port: Option<u16>,
}

/// Parses a discovery reply into (hostname, content server port, port).
pub fn parse_reply(text: &str) -> Option<(String, Option<u16>, u16)> {
    let rest = text.strip_prefix(REPLY_PREFIX)?;
    let (host, ports) = rest.rsplit_once(");")?;
    let (content, port) = ports.trim().split_once(',')?;
    Some((
        host.to_owned(),
        content.parse().ok(),
        port.trim().parse().ok()?,
    ))
}

/// The limited broadcast address; use explicit targets for unicast probes
/// (e.g. `127.0.0.1` when Calibre runs on the same machine).
pub fn default_targets() -> Vec<IpAddr> {
    vec![IpAddr::V4(Ipv4Addr::BROADCAST)]
}

/// Probes every broadcast port on each target and collects replies until
/// `timeout` passes.
pub fn discover(timeout: Duration, targets: &[IpAddr]) -> io::Result<Vec<CalibreServer>> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    sock.set_broadcast(true)?;
    let mut sent = 0;
    for target in targets {
        for port in BROADCAST_PORTS {
            match sock.send_to(b"hello", (*target, port)) {
                Ok(_) => sent += 1,
                Err(e) => log::debug!("probe to {target}:{port} failed: {e}"),
            }
        }
    }
    if sent == 0 {
        return Err(io::Error::other("could not send any discovery probe"));
    }

    let deadline = Instant::now() + timeout;
    let mut found: Vec<CalibreServer> = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        sock.set_read_timeout(Some(left))?;
        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                break
            }
            Err(e) => return Err(e),
        };
        let text = String::from_utf8_lossy(&buf[..n]);
        let Some((hostname, content_server_port, port)) = parse_reply(&text) else {
            log::debug!("ignoring non-calibre reply from {from}: {text:?}");
            continue;
        };
        let server = CalibreServer {
            addr: from.ip(),
            hostname,
            port,
            content_server_port,
        };
        if !found
            .iter()
            .any(|s| s.addr == server.addr && s.port == server.port)
        {
            log::debug!("found {server:?}");
            found.push(server);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_replies() {
        assert_eq!(
            parse_reply("calibre wireless device client (on calibre-host);8080,9090"),
            Some(("calibre-host".into(), Some(8080), 9090))
        );
        // The content server port is empty when it is not configured.
        assert_eq!(
            parse_reply("calibre wireless device client (on host (2));,9090"),
            Some(("host (2)".into(), None, 9090))
        );
        assert_eq!(parse_reply("something else"), None);
        assert_eq!(
            parse_reply("calibre wireless device client (on h);8080"),
            None
        );
    }

    #[test]
    fn discovers_a_fake_calibre_on_loopback() {
        // Bind one of the real ports if free; skip quietly if not (e.g. a real
        // Calibre is running on this machine).
        let Some(listener) = BROADCAST_PORTS
            .iter()
            .find_map(|p| UdpSocket::bind(("127.0.0.1", *p)).ok())
        else {
            return;
        };
        let handle = std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            let (_, from) = listener.recv_from(&mut buf).unwrap();
            listener
                .send_to(b"calibre wireless device client (on fake);,9191", from)
                .unwrap();
        });
        let found = discover(Duration::from_millis(500), &["127.0.0.1".parse().unwrap()]).unwrap();
        handle.join().unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].hostname, "fake");
        assert_eq!(found[0].port, 9191);
    }
}
