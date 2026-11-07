//! End-to-end tests for the in-memory dev server, over raw TCP.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use reuben_core::output::{Output, OutputFile};
use reuben_core::serve::DevServer;

fn output(files: &[(&str, &str)]) -> Output {
    let mut out = Output::default();
    for (k, v) in files {
        out.files
            .insert((*k).into(), OutputFile::Bytes(v.as_bytes().to_vec()));
    }
    out
}

fn get(addr: SocketAddr, method: &str, path: &str) -> (u16, String, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, head.to_ascii_lowercase(), body.to_string())
}

fn site() -> Output {
    output(&[
        ("index.html", "<html><body>home</body></html>"),
        ("a/index.html", "<p>a</p>"),
        ("style.css", "p{color:red}"),
        ("404.html", "<body>gone</body>"),
    ])
}

#[test]
fn serves_pages_assets_and_404() {
    let server = DevServer::start("127.0.0.1:0", site()).unwrap();
    let addr = server.local_addr();

    let (status, head, body) = get(addr, "GET", "/");
    assert_eq!(status, 200);
    assert!(head.contains("content-type: text/html; charset=utf-8"));
    assert!(body.contains("home<script src=\"/__reuben/live.js\"></script></body>"));

    let (status, head, body) = get(addr, "GET", "/style.css?v=1");
    assert_eq!(status, 200);
    assert!(head.contains("content-type: text/css"));
    assert_eq!(body, "p{color:red}");

    let (status, _, body) = get(addr, "GET", "/missing");
    assert_eq!(status, 404);
    assert!(body.contains("gone"));

    let (status, head, _) = get(addr, "GET", "/a?x=1");
    assert_eq!(status, 301);
    assert!(head.contains("location: /a/?x=1"));

    let (status, _, body) = get(addr, "HEAD", "/");
    assert_eq!(status, 200);
    assert!(body.is_empty());

    let (status, _, body) = get(addr, "GET", "/__reuben/live.js");
    assert_eq!(status, 200);
    assert!(body.contains("EventSource"));

    let (status, _, _) = get(addr, "POST", "/");
    assert_eq!(status, 405);
}

#[test]
fn copy_entries_are_read_at_request_time() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let src = tmp.path().join("f.css");
    std::fs::write(&src, "one").unwrap();
    let mut out = Output::default();
    out.files
        .insert("f.css".into(), OutputFile::Copy(src.clone()));
    let server = DevServer::start("127.0.0.1:0", out).unwrap();
    assert_eq!(get(server.local_addr(), "GET", "/f.css").2, "one");
    std::fs::write(&src, "two").unwrap();
    assert_eq!(get(server.local_addr(), "GET", "/f.css").2, "two");
}

#[test]
fn traversal_is_404() {
    let server = DevServer::start("127.0.0.1:0", Output::default()).unwrap();
    let (status, _, body) = get(server.local_addr(), "GET", "/../Cargo.toml");
    assert_eq!(status, 404);
    assert!(!body.contains("[package]"));
}

fn read_until(s: &mut TcpStream, needle: &str, within: Duration) -> String {
    s.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let deadline = Instant::now() + within;
    let mut acc = String::new();
    let mut buf = [0u8; 1024];
    while Instant::now() < deadline && !acc.contains(needle) {
        if let Ok(n) = s.read(&mut buf) {
            if n == 0 {
                break;
            }
            acc.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    }
    acc
}

#[test]
fn replace_sends_reload_event() {
    let server = DevServer::start("127.0.0.1:0", output(&[("index.html", "old")])).unwrap();
    let addr = server.local_addr();
    let mut sse = TcpStream::connect(addr).unwrap();
    write!(sse, "GET /__reuben/live HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let head = read_until(&mut sse, "\r\n\r\n", Duration::from_secs(2));
    assert!(head.contains("text/event-stream"), "{head}");

    server.replace(output(&[("index.html", "new")]));
    let got = read_until(&mut sse, "event: reload", Duration::from_secs(2));
    assert!(got.contains("event: reload"), "{got}");
    assert!(get(addr, "GET", "/").2.starts_with("new"));
}

#[test]
fn notify_error_keeps_output_and_does_not_reload() {
    let server = DevServer::start("127.0.0.1:0", output(&[("index.html", "kept")])).unwrap();
    let addr = server.local_addr();
    let mut sse = TcpStream::connect(addr).unwrap();
    write!(sse, "GET /__reuben/live HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    read_until(&mut sse, "\r\n\r\n", Duration::from_secs(2));
    server.notify_error();
    let got = read_until(&mut sse, "event: reload", Duration::from_millis(500));
    assert!(!got.contains("event: reload"));
    assert!(get(addr, "GET", "/").2.starts_with("kept"));
}

#[test]
fn drop_closes_streams_and_port() {
    let server = DevServer::start("127.0.0.1:0", Output::default()).unwrap();
    let addr = server.local_addr();
    let mut sse = TcpStream::connect(addr).unwrap();
    write!(sse, "GET /__reuben/live HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    read_until(&mut sse, "\r\n\r\n", Duration::from_secs(2));
    drop(server);
    let mut buf = [0u8; 256];
    sse.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    loop {
        match sse.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
    assert!(TcpStream::connect(addr).is_err());
}
