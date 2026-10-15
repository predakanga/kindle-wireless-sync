//! Replays anonymized recordings of real Calibre sessions (see
//! `tools/anonymize_transcript.py`) and checks that our replies match what
//! the client sent when they were recorded.

use std::io::Cursor;

use kws::library::Library;
use kws::protocol::{read_message, write_message, Connection, Opcode};
use kws::session::{self, Admission, DeviceConfig, SessionEnd};
use serde_json::{json, Value};

fn opcode_named(name: &str) -> Opcode {
    (0..=22)
        .filter_map(Opcode::from_code)
        .find(|op| op.to_string() == name)
        .unwrap_or_else(|| panic!("unknown opcode name {name}"))
}

/// Blanks values that legitimately differ between runs.
fn normalize(mut v: Value) -> Value {
    if let Some(obj) = v.as_object_mut() {
        for key in ["free_space_on_device", "total_space_on_device"] {
            if obj.contains_key(key) {
                obj.insert(key.into(), json!(0));
            }
        }
        for key in ["version", "device_version", "ccVersionNumber"] {
            if obj.contains_key(key) {
                obj.insert(key.into(), json!("VERSION"));
            }
        }
    }
    v
}

struct RecordedSession {
    input: Vec<u8>,
    expected: Vec<(Opcode, Value)>,
    library_uuid: String,
}

fn load_sessions(fixture: &str) -> Vec<RecordedSession> {
    let mut sessions: Vec<RecordedSession> = Vec::new();
    for line in fixture.lines().filter(|l| !l.trim().is_empty()) {
        let e: Value = serde_json::from_str(line).unwrap();
        let dir = e["dir"].as_str().unwrap();
        if dir == "session-start" {
            sessions.push(RecordedSession {
                input: Vec::new(),
                expected: Vec::new(),
                library_uuid: String::new(),
            });
            continue;
        }
        let s = sessions.last_mut().expect("entry before session-start");
        match dir {
            "recv" => {
                let op = opcode_named(e["op"].as_str().unwrap());
                if op == Opcode::GetInitializationInfo {
                    s.library_uuid = e["payload"]["currentLibraryUUID"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                }
                write_message(&mut s.input, op, &e["payload"]).unwrap();
            }
            "recv-raw" => {
                let len = e["len"].as_u64().unwrap() as usize;
                s.input.extend(std::iter::repeat_n(b'x', len));
            }
            "send" => s.expected.push((
                opcode_named(e["op"].as_str().unwrap()),
                normalize(e["payload"].clone()),
            )),
            other => panic!("unexpected transcript entry {other}"),
        }
    }
    sessions
}

#[test]
fn replays_calibre_9_16_sessions() {
    let fixture = include_str!("fixtures/calibre-9.16-session.jsonl");
    let tmp = tempfile::tempdir().unwrap();
    let mut lib = Library::open(&tmp.path().join("books"), &tmp.path().join("state")).unwrap();
    let cfg = DeviceConfig {
        device_name: "Kindle (kws dev)".into(),
        ..Default::default()
    };

    let sessions = load_sessions(fixture);
    assert_eq!(sessions.len(), 2);
    let mut ends = Vec::new();
    for (i, s) in sessions.into_iter().enumerate() {
        let mut out = Vec::new();
        let end = {
            let mut conn = Connection::new(Cursor::new(s.input), &mut out);
            let uuid = s.library_uuid.clone();
            session::run(&mut conn, &mut lib, &cfg, &mut |info| {
                if info.library_uuid == uuid {
                    Admission::Accept { password: None }
                } else {
                    Admission::Reject
                }
            })
            .unwrap()
        };
        ends.push(end);

        let mut cur = Cursor::new(out);
        let mut actual = Vec::new();
        while let Some(m) = read_message(&mut cur).unwrap() {
            actual.push((m.opcode().unwrap(), normalize(m.payload)));
        }
        assert_eq!(actual.len(), s.expected.len(), "session {i}: reply count");
        for (n, (a, e)) in actual.iter().zip(&s.expected).enumerate() {
            assert_eq!(a, e, "session {i}, reply {n}");
        }
    }
    // The first session was ejected by Calibre; the second was cut off by
    // the client exiting.
    assert_eq!(ends, vec![SessionEnd::Ejected, SessionEnd::Closed]);
    // Two of the three books sent remain; one was deleted.
    assert_eq!(lib.books().len(), 2);
}
