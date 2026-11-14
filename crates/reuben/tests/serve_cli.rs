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

fn spawn(root: &Path, extra: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_reuben"))
        .args(["serve", "--port", "0", "--root"])
        .arg(root)
        .args(extra)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn start(root: &Path) -> Serve {
    start_with(root, &[])
}

fn start_with(root: &Path, extra: &[&str]) -> Serve {
    let mut child = spawn(root, extra);
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

fn serve_stderr(serve: &Serve) -> String {
    serve.stderr.lock().unwrap().clone()
}

/// Waits for the next `Rebuilt` line, so the rebuild that saw the last write, and its watch update, are done.
fn wait_rebuilt(serve: &Serve) {
    let end = Instant::now() + Duration::from_secs(20);
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        match serve.stdout.recv_timeout(left) {
            Ok(line) if line.starts_with("Rebuilt ") => return,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    panic!("no rebuild line; stderr: {}", serve_stderr(serve));
}

#[test]
fn serve_accepts_jobs() {
    let tmp = site("Hello\n");
    let serve = start_with(tmp.path(), &["--jobs", "2"]);
    assert!(body_contains(serve.port, "Hello"));
}

fn stderr_contains(serve: &Serve, needle: &str) -> bool {
    serve.stderr.lock().unwrap().contains(needle)
}

#[test]
fn serve_watches_a_new_data_dir() {
    let tmp =
        site("T={{ site.title }} D={{ if data.contains(\"x\") { data.x.v } else { \"none\" } }}");
    let serve = start(tmp.path());
    assert!(
        body_contains(serve.port, "D=none"),
        "{}",
        serve_stderr(&serve)
    );
    // `data/` did not exist at startup. Nothing but the data file changes.
    tmp.child("data/x.json")
        .write_str("{\"v\": \"alpha\"}")
        .unwrap();
    assert!(
        wait_until(Duration::from_secs(20), || body_contains(
            serve.port, "D=alpha"
        )),
        "{}",
        serve_stderr(&serve)
    );
    wait_rebuilt(&serve);
    tmp.child("data/x.json")
        .write_str("{\"v\": \"beta\"}")
        .unwrap();
    assert!(wait_until(Duration::from_secs(20), || body_contains(
        serve.port, "D=beta"
    )));
}

#[test]
fn serve_watches_reuben_toml() {
    let tmp = site("T={{ site.title }}");
    let serve = start(tmp.path());
    tmp.child("reuben.toml")
        .write_str("title = \"Configured\"\n")
        .unwrap();
    assert!(
        wait_until(Duration::from_secs(20), || body_contains(
            serve.port,
            "T=Configured"
        )),
        "{}",
        serve_stderr(&serve)
    );
}

#[test]
fn serve_recovers_from_a_broken_include() {
    let tmp = site("T={{ site.title }}");
    tmp.child("reuben.toml")
        .write_str("include = [\"conf2/*.toml\"]\n")
        .unwrap();
    tmp.child("conf2").create_dir_all().unwrap();
    let serve = start(tmp.path());
    tmp.child("conf2/a.toml").write_str("title = \n").unwrap();
    assert!(
        wait_until(Duration::from_secs(20), || stderr_contains(
            &serve, "error:"
        )),
        "broken include was not noticed"
    );
    tmp.child("conf2/a.toml")
        .write_str("title = \"Included\"\n")
        .unwrap();
    assert!(
        wait_until(Duration::from_secs(20), || body_contains(
            serve.port,
            "T=Included"
        )),
        "{}",
        serve_stderr(&serve)
    );
}

#[test]
fn serve_notices_an_include_dir_created_later() {
    let tmp = site("T={{ site.title }}");
    tmp.child("reuben.toml")
        .write_str("include = [\"conf3/*.toml\"]\n")
        .unwrap();
    let serve = start(tmp.path());
    tmp.child("conf3/a.toml")
        .write_str("title = \"Three\"\n")
        .unwrap();
    assert!(
        wait_until(Duration::from_secs(20), || body_contains(
            serve.port, "T=Three"
        )),
        "{}",
        serve_stderr(&serve)
    );
    wait_rebuilt(&serve);
    tmp.child("conf3/a.toml")
        .write_str("title = \"Four\"\n")
        .unwrap();
    assert!(wait_until(Duration::from_secs(20), || body_contains(
        serve.port, "T=Four"
    )));
}

#[test]
fn serve_ignores_editor_swap_files() {
    let tmp = site("Hello");
    let serve = start(tmp.path());
    tmp.child("content/.index.md.swp")
        .write_str("junk")
        .unwrap();
    tmp.child("content/index.md~").write_str("junk").unwrap();
    thread::sleep(Duration::from_millis(1500));
    assert!(
        serve.stdout.try_recv().is_err(),
        "swap files caused a rebuild"
    );
    // A real edit still goes through afterwards.
    tmp.child("content/index.md")
        .write_str(&page("Real"))
        .unwrap();
    assert!(wait_until(Duration::from_secs(20), || body_contains(
        serve.port, "Real"
    )));
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
    tmp.child("content/index.md")
        .write_str(&page("Again"))
        .unwrap();
    assert!(wait_until(Duration::from_secs(20), || body_contains(
        serve.port, "Again"
    )));
    wait_rebuilt(&serve);
    tmp.child("content/index.md")
        .write_str(&page(BROKEN))
        .unwrap();
    assert!(wait_until(Duration::from_secs(20), || stderr_contains(
        &serve, "error:"
    )));
    assert!(!tmp.path().join("dist").exists());
    let names: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["content"]);
}

#[test]
fn serve_rebuild_reports_findings() {
    let tmp = site("# T\n\nHello\n");
    let serve = start(tmp.path());
    tmp.child("content/.index.md.tmp")
        .write_str(&page("# T\n\n[click here](/) Goodbye"))
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
        serve_stderr(&serve)
    );
    wait_rebuilt(&serve);
    assert!(
        wait_until(Duration::from_secs(5), || stderr_contains(
            &serve,
            "warning[link-text]"
        )),
        "stderr: {}",
        serve_stderr(&serve)
    );
}
