//! End-to-end tests for `reuben serve`: spawn the binary, talk HTTP to it, change files under it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use assert_fs::TempDir;
use assert_fs::prelude::*;

/// A running `reuben serve` that is killed and reaped when dropped.
struct Serve {
    child: Child,
    port: u16,
    stderr: Arc<Mutex<String>>,
    stdout: Receiver<String>,
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn drain<R: Read + Send + 'static>(r: R, on_line: impl Fn(String) + Send + 'static) {
    thread::spawn(move || {
        for line in BufReader::new(r).lines().map_while(Result::ok) {
            on_line(line);
        }
    });
}

fn spawn(root: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_reuben"))
        .args(["serve", "--port", "0", "--root"])
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn start(root: &Path) -> Serve {
    let mut child = spawn(root);
    let stderr = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&stderr);
    drain(child.stderr.take().unwrap(), move |l| {
        let mut s = sink.lock().unwrap();
        s.push_str(&l);
        s.push('\n');
    });
    let (tx, stdout) = mpsc::channel();
    drain(child.stdout.take().unwrap(), move |l| {
        let _ = tx.send(l);
    });
    let mut serve = Serve {
        child,
        port: 0,
        stderr,
        stdout,
    };
    let line = serve
        .stdout
        .recv_timeout(Duration::from_secs(60))
        .expect("serve printed nothing");
    let port = line
        .strip_prefix("Serving at http://127.0.0.1:")
        .unwrap_or_else(|| panic!("unexpected first line: {line}"));
    serve.port = port.parse().unwrap();
    serve
}

fn get(port: u16, path: &str) -> Option<(u16, String)> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut raw = String::new();
    s.read_to_string(&mut raw).ok()?;
    let status = raw.split(' ').nth(1)?.parse().ok()?;
    let body = raw.split_once("\r\n\r\n")?.1.to_string();
    Some((status, body))
}

fn wait_until(timeout: Duration, mut ok: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    ok()
}

fn page(body: &str) -> String {
    format!("---\ntitle: T\n---\n{body}\n")
}

const BROKEN: &str = "{{ nope( }}";

fn site(body: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    tmp.child("content/index.md")
        .write_str(&page(body))
        .unwrap();
    tmp
}

fn body_contains(port: u16, needle: &str) -> bool {
    get(port, "/").is_some_and(|(_, b)| b.contains(needle))
}

#[test]
fn serve_rebuilds_on_change() {
    let tmp = site("Hello\n");
    let serve = start(tmp.path());
    assert!(body_contains(serve.port, "Hello"));
    // Editors save by writing a temp file and renaming it over the original.
    tmp.child("content/.index.md.tmp")
        .write_str(&page("Goodbye"))
        .unwrap();
    std::fs::rename(
        tmp.path().join("content/.index.md.tmp"),
        tmp.path().join("content/index.md"),
    )
    .unwrap();
    assert!(
        wait_until(Duration::from_secs(20), || body_contains(
            serve.port, "Goodbye"
        )),
        "stderr: {}",
        serve.stderr.lock().unwrap()
    );
    let line = serve
        .stdout
        .recv_timeout(Duration::from_secs(5))
        .expect("rebuild line");
    assert!(
        line.starts_with("Rebuilt ") && line.contains(" page"),
        "{line}"
    );
}

#[test]
fn serve_picks_up_a_new_data_dir_and_config_change() {
    let tmp = site("Hello\n");
    let serve = start(tmp.path());
    tmp.child("reuben.toml")
        .write_str("title = \"T\"\n")
        .unwrap();
    tmp.child("content/two.md")
        .write_str(&page("Second"))
        .unwrap();
    assert!(wait_until(Duration::from_secs(20), || {
        get(serve.port, "/two/").is_some_and(|(s, b)| s == 200 && b.contains("Second"))
    }));
    // `data/` did not exist at startup; creating it must start being watched.
    tmp.child("data/x.json").write_str("{}").unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    tmp.child("content/two.md")
        .write_str(&page("Third"))
        .unwrap();
    assert!(wait_until(Duration::from_secs(20), || {
        get(serve.port, "/two/").is_some_and(|(_, b)| b.contains("Third"))
    }));
}

#[test]
fn serve_keeps_last_good_output_on_error() {
    let tmp = site("Hello\n");
    let serve = start(tmp.path());
    tmp.child("content/index.md")
        .write_str(&page(BROKEN))
        .unwrap();
    assert!(
        wait_until(Duration::from_secs(20), || serve
            .stderr
            .lock()
            .unwrap()
            .contains("error:")),
        "no error shown"
    );
    assert!(body_contains(serve.port, "Hello"));
    assert!(serve.stderr.lock().unwrap().contains("Found 1 error"));
    // Fixing the page recovers.
    tmp.child("content/index.md")
        .write_str(&page("Fixed"))
        .unwrap();
    assert!(wait_until(Duration::from_secs(20), || body_contains(
        serve.port, "Fixed"
    )));
}

#[test]
fn serve_exits_1_when_first_build_fails() {
    let tmp = site(BROKEN);
    let out = Command::new(env!("CARGO_BIN_EXE_reuben"))
        .args(["serve", "--port", "0", "--root"])
        .arg(tmp.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("error:"));
}

#[test]
fn serve_exits_1_when_port_is_taken() {
    let tmp = site("Hello\n");
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let out = Command::new(env!("CARGO_BIN_EXE_reuben"))
        .args(["serve", "--port", &port.to_string(), "--root"])
        .arg(tmp.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(&port.to_string()) && err.contains("error:"),
        "{err}"
    );
}

#[test]
fn serve_writes_nothing_to_the_site() {
    let tmp = site("Hello\n");
    let serve = start(tmp.path());
    assert!(body_contains(serve.port, "Hello"));
    assert!(!tmp.path().join("dist").exists());
    let names: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["content"]);
}
