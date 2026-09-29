//! A whole synthetic session through the real `apw daemon` binary, then the key
//! paths of the manifest it wrote against the manifest the Python daemon wrote for
//! the same session (`tests/fixtures/parity/manifest_key_paths.json`).
//!
//! Compared: every object key path (list indexes collapse to `[]`), the value
//! kinds seen at each path, and the insertion order of each object's keys. Never
//! compared: values (ids, hashes, paths, timestamps, signatures, counts), which
//! are run-specific. Any key one implementation emits and the other does not is a
//! failure.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, UdpSocket};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const SAMPLE_RATE: u32 = 44_100;
const WINDOW: usize = 4096;

/// `scripts/synthetic_rehearsal.py::_demo_samples`.
fn demo_samples(window_count: usize) -> Vec<f64> {
    let frequencies = [196.0, 293.66, 440.0, 659.25, 329.63, 246.94, 523.25];
    let levels = [0.12, 0.30, 0.18, 0.38, 0.09, 0.26, 0.16, 0.34];
    let quarter_shape = [0.55, 1.0, 0.72, 0.88];
    let mut samples = Vec::with_capacity(window_count * WINDOW);
    for window_index in 0..window_count {
        let frequency = frequencies[window_index % frequencies.len()];
        let level = levels[(window_index * 3) % levels.len()];
        for offset in 0..WINDOW {
            let absolute = (window_index * WINDOW + offset) as f64;
            let shape = quarter_shape[(offset * 4 / WINDOW).min(3)];
            let rate = f64::from(SAMPLE_RATE);
            let fundamental = (2.0 * std::f64::consts::PI * frequency * absolute / rate).sin();
            let harmonic =
                0.23 * (2.0 * std::f64::consts::PI * frequency * 2.01 * absolute / rate).sin();
            samples.push(level * shape * (fundamental + harmonic));
        }
    }
    samples
}

fn write_wav(path: &Path, samples: &[f64]) {
    let data_len = (samples.len() * 2) as u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for value in samples {
        let scaled = (value * 32767.0).round_ties_even().clamp(-32768.0, 32767.0);
        bytes.extend_from_slice(&(scaled as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(number) => {
            let text = number.to_string();
            if text.contains(['.', 'e', 'E']) { "float" } else { "int" }
        }
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "object",
    }
}

type Paths = BTreeMap<String, (Vec<String>, Option<Vec<String>>)>;

fn key_paths(value: &Value, path: &str, out: &mut Paths) {
    let entry = out
        .entry(if path.is_empty() { "/".to_owned() } else { path.to_owned() })
        .or_insert_with(|| (Vec::new(), None));
    let kind = kind_of(value).to_owned();
    if !entry.0.contains(&kind) {
        entry.0.push(kind);
        entry.0.sort();
    }
    match value {
        Value::Object(map) => {
            if entry.1.is_none() {
                entry.1 = Some(map.keys().cloned().collect());
            }
            for (key, child) in map {
                key_paths(child, &format!("{path}/{key}"), out);
            }
        }
        Value::Array(items) => {
            for child in items {
                key_paths(child, &format!("{path}[]"), out);
            }
        }
        _ => {}
    }
}

fn der(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    if content.len() < 0x80 {
        out.push(content.len() as u8);
    } else {
        out.extend([0x82, (content.len() >> 8) as u8, content.len() as u8]);
    }
    out.extend_from_slice(content);
    out
}

fn der_uint(magnitude: &[u8]) -> Vec<u8> {
    let mut body: Vec<u8> = magnitude.iter().copied().skip_while(|b| *b == 0).collect();
    if body.first().is_none_or(|b| b & 0x80 != 0) {
        body.insert(0, 0);
    }
    der(0x02, &body)
}

/// A granted TimeStampResp bound to the request's hash and nonce.
fn echoing_reply(request: &[u8]) -> Vec<u8> {
    let (hash, nonce) = apw_core::parse_timestamp_request(request).unwrap();
    let sha256 = [0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
    let mut algorithm = sha256.to_vec();
    algorithm.extend(der(0x05, &[]));
    let mut imprint = der(0x30, &algorithm);
    imprint.extend(der(0x04, &apw_core::python_from_hex(&hash).unwrap()));
    let mut tst = der(0x02, &[1]);
    tst.extend([0x06, 0x03, 0x2a, 0x03, 0x04]);
    tst.extend(der(0x30, &imprint));
    tst.extend(der_uint(&[9, 9]));
    tst.extend(der(0x18, b"20260928123000Z"));
    tst.extend(der_uint(&nonce));
    let mut encap = vec![0x06, 0x0b, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x10, 0x01, 0x04];
    encap.extend(der(0xA0, &der(0x04, &der(0x30, &tst))));
    let mut signed = der_uint(&[3]);
    signed.extend(der(0x31, &[]));
    signed.extend(der(0x30, &encap));
    let mut token = vec![0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02];
    token.extend(der(0xA0, &der(0x30, &signed)));
    let mut response = der(0x30, &der_uint(&[0]));
    response.extend(der(0x30, &token));
    der(0x30, &response)
}

/// A loopback TSA: answers every POST with a token echoing its hash and nonce.
fn spawn_tsa() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut received = Vec::new();
            let mut chunk = [0_u8; 1024];
            while let Ok(read) = stream.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                received.extend_from_slice(&chunk[..read]);
                let Some(split) = received.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                let head = String::from_utf8_lossy(&received[..split]).to_ascii_lowercase();
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .and_then(|value| value.trim().parse().ok())
                    .unwrap_or(0);
                if received.len() >= split + 4 + length {
                    let body = echoing_reply(&received[split + 4..split + 4 + length]);
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                    break;
                }
            }
        }
    });
    port
}

/// A loopback OpenTimestamps calendar: answers POST /cal/digest with a pending receipt.
fn spawn_calendar() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut received = Vec::new();
            let mut chunk = [0_u8; 1024];
            while let Ok(read) = stream.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                received.extend_from_slice(&chunk[..read]);
                let Some(split) = received.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                let head = String::from_utf8_lossy(&received[..split]).to_ascii_lowercase();
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .and_then(|value| value.trim().parse().ok())
                    .unwrap_or(0);
                if received.len() >= split + 4 + length {
                    let uri = format!("http://127.0.0.1:{port}/cal");
                    let mut body = vec![0xf0, 0x08];
                    body.extend([7_u8; 8]);
                    body.extend([0x08, 0x00, 0x83, 0xdf, 0xe3, 0x0d, 0x2e, 0xf9, 0x0c, 0x8e]);
                    body.extend([(uri.len() + 1) as u8, uri.len() as u8]);
                    body.extend(uri.as_bytes());
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                    break;
                }
            }
        }
    });
    port
}

fn free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn run_session(root: &Path, events: &[Value]) -> Value {
    for directory in ["samples", "exports", "manifests", "evidence"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    let port = free_udp_port();
    let tsa = format!("http://127.0.0.1:{}/tsa", spawn_tsa());
    let calendar = format!("http://127.0.0.1:{}/cal", spawn_calendar());
    let key = |name: &str| root.join(name);
    let mut child = Command::new(env!("CARGO_BIN_EXE_apw"))
        .arg("daemon")
        .args(["--port", &port.to_string()])
        .args(["--evidence-dir".as_ref(), root.join("evidence").as_os_str()])
        .args(["--sample-dir".as_ref(), root.join("samples").as_os_str()])
        .args(["--export-dir".as_ref(), root.join("exports").as_os_str()])
        .args(["--manifest-dir".as_ref(), root.join("manifests").as_os_str()])
        .args(["--session-id", "parity-session", "--stem-id", "synthetic-stem"])
        .args(["--source-category", "generator"])
        .args(["--time-anchor", &tsa])
        .args(["--ots-calendar", &calendar])
        .args(["--signing-key".as_ref(), key("local.key").as_os_str()])
        .args(["--portable-private-key".as_ref(), key("pp.key").as_os_str()])
        .args(["--portable-public-key".as_ref(), key("pub.key").as_os_str()])
        .args(["--provenance-store".as_ref(), key("prov").as_os_str()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    // The daemon binds its port after start-up; retry the first datagram.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut buffer = [0_u8; 8192];
    for (index, event) in events.iter().enumerate() {
        let payload = serde_json::to_vec(event).unwrap();
        loop {
            socket.send_to(&payload, ("127.0.0.1", port)).unwrap();
            match socket.recv_from(&mut buffer) {
                Ok((length, _)) => {
                    let ack: Value = serde_json::from_slice(&buffer[..length]).unwrap();
                    assert_eq!(ack["accepted"], true, "event {index} rejected: {ack}");
                    break;
                }
                Err(_) if index == 0 && Instant::now() < deadline => continue,
                Err(error) => panic!("no acknowledgement for event {index}: {error}"),
            }
        }
    }

    let source = demo_samples(40);
    let mut export: Vec<f64> = vec![0.0; 3 * WINDOW];
    export.extend(source.iter().map(|sample| sample * 0.58));
    write_wav(&root.join("exports/presenter_export.wav"), &export);

    let manifest_path = root.join("manifests/presenter_export_manifest.json");
    let deadline = Instant::now() + Duration::from_secs(60);
    let manifest = loop {
        if let Ok(bytes) = std::fs::read(&manifest_path) {
            if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                break value;
            }
        }
        assert!(Instant::now() < deadline, "the daemon wrote no manifest");
        std::thread::sleep(Duration::from_millis(200));
    };
    let _ = child.kill();
    let _ = child.wait();
    manifest
}

#[test]
fn the_rust_session_manifest_has_the_python_key_paths() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/parity");
    let events: Vec<Value> =
        serde_json::from_slice(&std::fs::read(fixtures.join("synthetic_events.json")).unwrap()).unwrap();
    let expected: Value =
        serde_json::from_slice(&std::fs::read(fixtures.join("manifest_key_paths.json")).unwrap()).unwrap();

    let directory = tempfile::tempdir().unwrap();
    let manifest = run_session(directory.path(), &events);
    let mut produced = Paths::new();
    key_paths(&manifest, "", &mut produced);

    let mut differences = Vec::new();
    let expected_paths = expected["paths"].as_object().unwrap();
    for (path, entry) in expected_paths {
        let kinds: Vec<String> = entry["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|kind| kind.as_str().unwrap().to_owned())
            .collect();
        let keys: Option<Vec<String>> = entry["keys"]
            .as_array()
            .map(|keys| keys.iter().map(|key| key.as_str().unwrap().to_owned()).collect());
        match produced.get(path) {
            None => differences.push(format!("PYTHON ONLY  {path}")),
            Some((rust_kinds, rust_keys)) => {
                if *rust_kinds != kinds {
                    differences.push(format!("KINDS        {path}: python {kinds:?} rust {rust_kinds:?}"));
                }
                if *rust_keys != keys {
                    differences.push(format!("KEY ORDER    {path}: python {keys:?} rust {rust_keys:?}"));
                }
            }
        }
    }
    for path in produced.keys().filter(|path| !expected_paths.contains_key(*path)) {
        differences.push(format!("RUST ONLY    {path}"));
    }
    assert!(
        differences.is_empty(),
        "{} key-path difference(s):\n{}",
        differences.len(),
        differences.join("\n")
    );
}
