use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const FIRST: &str = "10000000-0000-0000-0000-000000000001";
const SECOND: &str = "10000000-0000-0000-0000-000000000002";

fn guard(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_guard"))
        .args(args)
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .env("NO_PROXY", "*")
        .output()
        .unwrap()
}

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "guard-test-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// A real loopback HTTP server checks the executable's method, path and query contract.
fn server(responses: Vec<(String, u16, String)>) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        for (expected, status, body) in responses {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    error => panic!("Expected request {expected}: {error:?}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line.trim(), format!("GET {expected} HTTP/1.1"));
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    (url, handle)
}

fn detail(id: &str, body: &str) -> String {
    json!({"submissionId": id, "submittedAt": "2026-10-19T10:00:00Z", "outcome": "ACCEPTED", "resolvedChannel": "sms", "payload": {"body": body}, "attempts": [{"notificationId": "unrelated"}]}).to_string()
}

#[test]
fn offline_demo_produces_joinable_report_without_content() {
    let output = guard(&["audit", "--input", "fixtures/demo-history.json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["summary"]["submissions"], 6);
    assert_eq!(report["summary"]["findings"], 4);
    assert_eq!(report["summary"]["bodies_unavailable"], 2);
    assert_eq!(report["summary"]["estimated_sms_segments"], 4);
    assert_eq!(report["submissions"][0]["sms"]["segments"], 1);
    assert_eq!(report["submissions"][1]["sms"]["segments"], 2);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("firstName"));
    assert!(!text.contains("15550100001"));
}

#[test]
fn explain_identifies_encoding_and_findings() {
    let output = guard(&[
        "explain",
        "--input",
        "fixtures/demo-history.json",
        "--submission",
        SECOND,
    ]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("2 estimated segment(s)"));
    assert!(text.contains("SmsUnicode"));
    assert!(text.contains("SmsMultipart"));
}

#[test]
fn invalid_cli_arguments_exit_two() {
    for args in [
        vec!["audit", "--max-records", "0"],
        vec!["audit", "--url", "http://localhost", "--input", "unused"],
        vec!["explain"],
    ] {
        assert_eq!(guard(&args).status.code(), Some(2));
    }
}

#[test]
fn atomic_output_and_failure_preserve_the_previous_report() {
    let dir = Directory::new();
    let path = dir.0.join("audit.json");
    let output = guard(&[
        "audit",
        "--input",
        "fixtures/demo-history.json",
        "--output",
        path.to_str().unwrap(),
    ]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    let before = std::fs::read(&path).unwrap();
    assert!(serde_json::from_slice::<Value>(&before).is_ok());
    let output = guard(&[
        "audit",
        "--input",
        "fixtures/demo-history.json",
        "--max-records",
        "1",
        "--output",
        path.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(before, std::fs::read(&path).unwrap());
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
}

#[test]
fn refuses_overwriting_the_input_fixture() {
    let dir = Directory::new();
    let path = dir.0.join("input.json");
    let before = std::fs::read("fixtures/demo-history.json").unwrap();
    std::fs::write(&path, &before).unwrap();
    let output = guard(&[
        "audit",
        "--input",
        path.to_str().unwrap(),
        "--output",
        path.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(before, std::fs::read(path).unwrap());
}

#[test]
fn follows_opaque_cursor_then_reads_each_detail_once() {
    let (url, task) = server(vec![
        (
            "/context/api/submissions?limit=200".into(),
            200,
            json!({"items": [{"submissionId": FIRST}], "nextCursor": "opaque + /="}).to_string(),
        ),
        (
            "/context/api/submissions?limit=200&cursor=opaque+%2B+%2F%3D".into(),
            200,
            json!({"items": [{"submissionId": SECOND}], "nextCursor": null}).to_string(),
        ),
        (
            format!("/context/api/submissions/{FIRST}"),
            200,
            detail(FIRST, "Hello {{firstName}}"),
        ),
        (
            format!("/context/api/submissions/{SECOND}"),
            200,
            detail(SECOND, &("a".repeat(80) + "😀")),
        ),
    ]);
    let output = guard(&["audit", "--url", &(url + "/context/")]);
    task.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["summary"]["submissions"], 2);
    assert_eq!(report["summary"]["estimated_sms_segments"], 3);
    assert_eq!(report["summary"]["findings"], 3);
}

#[test]
fn explain_reads_only_the_requested_detail() {
    let (url, task) = server(vec![(
        format!("/api/submissions/{FIRST}"),
        200,
        detail(FIRST, "Hi"),
    )]);
    let output = guard(&["explain", "--url", &url, "--submission", FIRST]);
    task.join().unwrap();
    assert!(output.status.success());
}

#[test]
fn refuses_malformed_pagination_and_duplicate_ids() {
    for page in [
        json!({"items": []}),
        json!({"items": [], "nextCursor": 12}),
        json!({"items": [], "nextCursor": "cursor"}),
        json!({"items": [{"submissionId": FIRST}, {"submissionId": FIRST}], "nextCursor": null}),
    ] {
        let (url, task) = server(vec![(
            "/api/submissions?limit=200".into(),
            200,
            page.to_string(),
        )]);
        let output = guard(&["audit", "--url", &url]);
        task.join().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn refuses_repeated_cursors_and_record_overflow() {
    let (url, task) = server(vec![
        (
            "/api/submissions?limit=200".into(),
            200,
            json!({"items": [{"submissionId": FIRST}], "nextCursor": "same"}).to_string(),
        ),
        (
            "/api/submissions?limit=200&cursor=same".into(),
            200,
            json!({"items": [{"submissionId": SECOND}], "nextCursor": "same"}).to_string(),
        ),
    ]);
    let output = guard(&["audit", "--url", &url]);
    task.join().unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("Repeated history cursor"));
    let (url, task) = server(vec![(
        "/api/submissions?limit=200".into(),
        200,
        json!({"items": [{"submissionId": FIRST}, {"submissionId": SECOND}], "nextCursor": null})
            .to_string(),
    )]);
    let output = guard(&["audit", "--url", &url, "--max-records", "1"]);
    task.join().unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("exceeds"));
    assert!(output.stdout.is_empty());
}

#[test]
fn http_errors_and_redirects_do_not_become_empty_success_reports() {
    for status in [302, 404, 503] {
        let (url, task) = server(vec![(
            "/api/submissions?limit=200".into(),
            status,
            "{\"message\":\"sensitive server text\"}".into(),
        )]);
        let output = guard(&["audit", "--url", &url]);
        task.join().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("sensitive"));
    }
}

#[test]
fn mismatched_detail_ids_and_oversized_responses_fail_closed() {
    let (url, task) = server(vec![(
        format!("/api/submissions/{FIRST}"),
        200,
        detail(SECOND, "Hi"),
    )]);
    let output = guard(&["explain", "--url", &url, "--submission", FIRST]);
    task.join().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let (url, task) = server(vec![(
        "/api/submissions?limit=200".into(),
        200,
        " ".repeat(2 * 1024 * 1024 + 1),
    )]);
    let output = guard(&["audit", "--url", &url]);
    task.join().unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("exceeds 2 MiB"));
}

#[test]
fn empty_history_is_a_successful_zero_report() {
    let (url, task) = server(vec![(
        "/api/submissions?limit=200".into(),
        200,
        json!({"items": [], "nextCursor": null}).to_string(),
    )]);
    let output = guard(&["audit", "--url", &url]);
    task.join().unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["summary"]["submissions"], 0);
}
