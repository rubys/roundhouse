//! The real threaded WebSocket runtime over loopback sockets. The ordinary
//! test uses CRuby OS bindings; the ignored lane compiles the same fixture
//! with Spinel, including its green-thread IO waits and native socket writes.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn until(mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    false
}

fn frame(input: &mut BufReader<TcpStream>) -> std::io::Result<(u8, Vec<u8>)> {
    let mut head = [0; 2];
    input.read_exact(&mut head)?;
    assert_eq!(head[1] & 128, 0, "server frames are unmasked");
    let mut n = (head[1] & 127) as usize;
    if n == 126 {
        let mut bytes = [0; 2];
        input.read_exact(&mut bytes)?;
        n = u16::from_be_bytes(bytes) as usize;
    } else if n == 127 {
        let mut bytes = [0; 8];
        input.read_exact(&mut bytes)?;
        n = u64::from_be_bytes(bytes) as usize;
    }
    assert!(n < 131_072, "bounded synthetic frame");
    let mut payload = vec![0; n];
    input.read_exact(&mut payload)?;
    Ok((head[0] & 15, payload))
}

fn masked(op: u8, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() < 126);
    let mask = [1, 2, 3, 4];
    let mut out = vec![128 | op, 128 | payload.len() as u8];
    out.extend(mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

fn run_case(mut command: Command, mode: &str) -> Vec<String> {
    let reservation = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let mut worker = Worker(
        command
            .arg(port.to_string())
            .arg(mode)
            .env("SPINEL_WORKERS", "2")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start fixture"),
    );
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let stdout = worker.0.stdout.take().unwrap();
    let stderr = worker.0.stderr.take().unwrap();
    let seen = lines.clone();
    let out_reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            seen.lock().unwrap().push(line.unwrap());
        }
    });
    let seen = errors.clone();
    let err_reader = thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            seen.lock().unwrap().push(line.unwrap());
        }
    });
    let contains = |text: &str| lines.lock().unwrap().iter().any(|line| line == text);
    let mut failed = Vec::new();
    macro_rules! check {
        ($name:expr, $condition:expr) => {
            if !$condition {
                failed.push(format!("{mode}:{}", $name));
            }
        };
    }
    if !until(|| contains("LISTENING")) {
        return vec![format!("{mode}:fixture_startup")];
    }
    let mut socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut input = BufReader::new(socket.try_clone().unwrap());
    loop {
        let mut line = String::new();
        input.read_line(&mut line).unwrap();
        if line == "\r\n" {
            break;
        }
        assert!(!line.is_empty(), "fixture handshake ended early");
    }
    check!(
        "open_frame",
        frame(&mut input).unwrap() == (1, b"ready".to_vec())
    );
    if mode == "client" || mode == "client_race" || mode == "race" {
        if mode == "race" {
            thread::sleep(Duration::from_millis(150));
        }
        let mut bytes = masked(8, b"\x0f\xa1peer");
        // Two frames already in the accumulator must still dispatch one close.
        bytes.extend(masked(8, b"\x0f\xa1peer"));
        let _ = socket.write_all(&bytes);
    } else if mode == "message_error" {
        socket.write_all(&masked(1, b"explode")).unwrap();
    } else if mode == "eof" {
        socket.shutdown(std::net::Shutdown::Write).unwrap();
    }
    if mode == "concurrent" {
        let mut payloads = Vec::new();
        for _ in 0..16 {
            let (op, payload) = frame(&mut input).unwrap();
            check!("concurrent_opcode", op == 1);
            payloads.push(payload);
        }
        let mut expected = Vec::new();
        for prefix in ["A", "B"] {
            for i in 0..8 {
                expected.push(format!("{prefix}{i}:{}", prefix.repeat(65536)).into_bytes());
            }
        }
        payloads.sort();
        expected.sort();
        check!("concurrent_frames_intact", payloads == expected);
    }
    let mut close = None;
    if !["open_error", "message_error", "eof"].contains(&mode) {
        let (op, payload) = frame(&mut input).unwrap();
        check!("close_frame", op == 8);
        if mode == "client" {
            check!("peer_close_payload", payload == b"\x0f\xa1peer");
        } else if mode != "race" {
            check!("server_close_payload", payload == b"\x0f\xa0local");
        }
        close = Some(payload);
    }
    let received_reply = close.is_some() && !["server_eof", "server_silent"].contains(&mode);
    if close.is_some() && !["client", "client_race", "race"].contains(&mode) {
        thread::sleep(Duration::from_millis(100));
        check!(
            "no_callback_or_cleanup_before_peer_result",
            !lines
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.starts_with("CALLBACK ") || s.starts_with("RETIRED "))
        );
        if mode == "server_eof" {
            let _ = socket.shutdown(std::net::Shutdown::Write);
        } else if mode != "server_silent" {
            // Application messages while closing are ignored; the following
            // close still supplies the received handshake result.
            let mut bytes = masked(1, b"late application message");
            bytes.extend(masked(8, b"\x0f\xa1peer"));
            let _ = socket.write_all(&bytes);
        }
    }
    // A silent peer stays open throughout the bounded retirement check.
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline && !contains("NEXT") {
        thread::sleep(Duration::from_millis(5));
    }
    check!("cleanup_after_peer_result_once", contains("RETIRED 1 1 0"));
    if [
        "server",
        "server_eof",
        "server_silent",
        "close_error",
        "open_close",
    ]
    .contains(&mode)
    {
        check!("later_sends_refused", contains("AFTER_CLOSE -1 -1"));
    }
    if mode == "client_race" {
        check!(
            "application_frame_refused_after_peer_result",
            contains("RACE_WRITE -1")
        );
    }
    let event = if received_reply {
        "CALLBACK 4001 peer"
    } else {
        "CALLBACK 1006 "
    };
    check!("close_event_describes_received_result", contains(event));
    drop(input);
    drop(socket);
    check!("connection_finishes", until(|| contains("NEXT")));
    let mut next = TcpStream::connect(("127.0.0.1", port)).unwrap();
    next.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut response = String::new();
    next.read_to_string(&mut response).unwrap();
    check!(
        "worker_still_serves",
        response.starts_with("HTTP/1.0 200 ") && response.ends_with("alive")
    );
    check!(
        "worker_exits",
        until(|| worker.0.try_wait().unwrap().is_some())
    );
    out_reader.join().unwrap();
    err_reader.join().unwrap();
    check!(
        "callback_error_contained",
        !lines
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.starts_with("ESCAPED "))
    );
    let error_count = errors
        .lock()
        .unwrap()
        .iter()
        .filter(|s| s.contains("synthetic ") && s.contains("callback error"))
        .count();
    check!(
        "callback_error_reported_once",
        error_count == usize::from(mode.ends_with("_error"))
    );
    failed
}

fn modes() -> [&'static str; 12] {
    [
        "server",
        "server_eof",
        "server_silent",
        "client",
        "client_race",
        "race",
        "open_close",
        "concurrent",
        "open_error",
        "message_error",
        "close_error",
        "eof",
    ]
}

#[test]
fn rails_close_lifecycle_when_available() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for defaults in ["7.0", "8.1"] {
        let output = Command::new("ruby")
            .arg(root.join("tests/spinel_websocket_lifecycle_rails.rb"))
            .env("RH_RAILS_DEFAULTS", defaults)
            .output()
            .expect("run optional Rails oracle");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success()
                && (stdout.contains("PASS Rails close lifecycle")
                    || stdout.contains("SKIP Rails 8.1.4 Action Cable unavailable")),
            "Rails close oracle failed for defaults {defaults}"
        );
    }
}

#[test]
fn websocket_lifecycle_over_real_sockets() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut failed = Vec::new();
    for mode in modes() {
        let mut command = Command::new("ruby");
        command.arg(root.join("tests/spinel_websocket_lifecycle_cruby.rb"));
        failed.extend(run_case(command, mode));
    }
    assert!(
        failed.is_empty(),
        "{} failing contracts: {}",
        failed.len(),
        failed.join(", ")
    );
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL to its path"]
fn websocket_lifecycle_on_spinel() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = std::env::temp_dir().join(format!("roundhouse-ws-lifecycle-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    let binary = dir.join("lifecycle");
    let compiled = Command::new(std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into()))
        .arg(root.join("tests/spinel_websocket_lifecycle_native.rb"))
        .arg("-o")
        .arg(&binary)
        .current_dir(root)
        .output()
        .expect("compile native fixture");
    assert!(
        compiled.status.success(),
        "native fixture compilation failed ({} diagnostic lines)",
        String::from_utf8_lossy(&compiled.stderr).lines().count()
    );
    let mut failed = Vec::new();
    for mode in modes() {
        failed.extend(run_case(Command::new(&binary), mode));
    }
    assert!(
        failed.is_empty(),
        "{} failing native contracts: {}",
        failed.len(),
        failed.join(", ")
    );
}
