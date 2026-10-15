//! Wi-Fi state on the Kindle, from `com.lab126.wifid` over lipc.
//!
//! `cmState` is `CONNECTED` when Wi-Fi is usable. It changes to `NA`
//! when the screensaver starts, and goes through `READY` and `PENDING` while
//! reconnecting; `cmStateChange "<state>"` events announce each change. See
//! `docs/research/kindle-library-integration.md` (network-up events).

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

const SERVICE: &str = "com.lab126.wifid";

/// Current Wi-Fi state, or `None` if lipc isn't available (not a Kindle).
pub fn wifi_connected() -> Option<bool> {
    let out = Command::new("lipc-get-prop")
        .args([SERVICE, "cmState"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim() == "CONNECTED")
}

/// Parses an `lipc-wait-event` line such as `cmStateChange "CONNECTED"`.
fn parse_state_change(line: &str) -> Option<bool> {
    let rest = line.trim().strip_prefix("cmStateChange")?;
    Some(rest.trim().trim_matches('"') == "CONNECTED")
}

/// Calls `on_change(connected)` on every Wi-Fi state change, from a
/// background thread, for the life of the process. Restarts the event
/// listener if it exits.
pub fn watch_wifi(on_change: impl Fn(bool) + Send + 'static) {
    std::thread::spawn(move || {
        let mut last: Option<bool> = None;
        loop {
            let child = Command::new("lipc-wait-event")
                .args(["-m", SERVICE, "cmStateChange"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            match child {
                Ok(mut child) => {
                    let stdout = child.stdout.take().expect("piped stdout");
                    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                        if let Some(up) = parse_state_change(&line) {
                            if last != Some(up) {
                                last = Some(up);
                                on_change(up);
                            }
                        }
                    }
                    let _ = child.wait();
                    log::warn!("lipc-wait-event exited; restarting");
                }
                Err(e) => log::warn!("cannot run lipc-wait-event: {e}"),
            }
            std::thread::sleep(Duration::from_secs(5));
            // Resynchronise in case a change was missed while restarting.
            if let Some(up) = wifi_connected() {
                if last != Some(up) {
                    last = Some(up);
                    on_change(up);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_state_changes() {
        assert_eq!(
            parse_state_change("cmStateChange \"CONNECTED\" "),
            Some(true)
        );
        assert_eq!(parse_state_change("cmStateChange \"NA\""), Some(false));
        assert_eq!(parse_state_change("cmStateChange \"PENDING\""), Some(false));
        assert_eq!(parse_state_change("cmConnected"), None);
        assert_eq!(parse_state_change("signalStrength \"5/5\""), None);
    }
}
