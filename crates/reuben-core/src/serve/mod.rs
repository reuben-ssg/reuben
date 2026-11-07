//! In-memory dev server: serves an `Output` over HTTP and tells browsers to reload when it is replaced.

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tiny_http::{Header, Method, Request, Response, Server};

use crate::output::{Output, OutputFile};

mod http;

use http::{LIVE_EVENTS_PATH, LIVE_SCRIPT, LIVE_SCRIPT_PATH, Route};

const HEARTBEAT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(100);

struct Shared {
    output: RwLock<Arc<Output>>,
    clients: Mutex<Vec<Sender<&'static [u8]>>>,
    stopping: AtomicBool,
}

/// A running dev server. Dropping it stops the listener and ends every live-reload stream.
pub struct DevServer {
    shared: Arc<Shared>,
    server: Arc<Server>,
    addr: SocketAddr,
    thread: Option<JoinHandle<()>>,
}

impl DevServer {
    /// Binds `addr` (port `0` picks a free one) and serves `output` on background threads.
    pub fn start(addr: &str, output: Output) -> io::Result<DevServer> {
        let server = Server::http(addr).map_err(io::Error::other)?;
        let addr = server.server_addr().to_ip().ok_or_else(|| {
            io::Error::new(io::ErrorKind::Unsupported, "server is not on an IP socket")
        })?;
        let server = Arc::new(server);
        let shared = Arc::new(Shared {
            output: RwLock::new(Arc::new(output)),
            clients: Mutex::new(Vec::new()),
            stopping: AtomicBool::new(false),
        });
        let thread = {
            let (server, shared) = (Arc::clone(&server), Arc::clone(&shared));
            thread::spawn(move || accept_loop(&server, &shared))
        };
        Ok(DevServer {
            shared,
            server,
            addr,
            thread: Some(thread),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Swaps the served output and tells every connected browser to reload.
    pub fn replace(&self, output: Output) {
        *self
            .shared
            .output
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Arc::new(output);
        let mut clients = self
            .shared
            .clients
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clients.retain(|c| c.send(RELOAD_EVENT).is_ok());
    }

    /// Keeps the current output and sends no reload.
    pub fn notify_error(&self) {}
}

impl Drop for DevServer {
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        self.server.unblock();
        // Dropping the senders ends each stream's wait.
        self.shared
            .clients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

const RELOAD_EVENT: &[u8] = b"event: reload\ndata: reload\n\n";
const PING: &[u8] = b": ping\n\n";

fn accept_loop(server: &Server, shared: &Arc<Shared>) {
    while !shared.stopping.load(Ordering::SeqCst) {
        match server.recv_timeout(POLL) {
            Ok(Some(request)) => handle(request, shared),
            Ok(None) => {}
            Err(_) => break,
        }
    }
}

fn handle(request: Request, shared: &Arc<Shared>) {
    let method = request.method().clone();
    if method != Method::Get && method != Method::Head {
        let _ = request
            .respond(text(405, "method not allowed").with_header(header("Allow", "GET, HEAD")));
        return;
    }
    let url = request.url().to_string();
    let path = url.split(['?', '#']).next().unwrap_or("");
    let query = url.find('?').map_or("", |i| &url[i..]);

    if path == LIVE_EVENTS_PATH {
        if method == Method::Get {
            let shared = Arc::clone(shared);
            thread::spawn(move || stream_events(request, &shared));
        } else {
            let _ = request.respond(
                Response::from_data(Vec::new())
                    .with_header(header("Content-Type", "text/event-stream")),
            );
        }
        return;
    }
    if path == LIVE_SCRIPT_PATH {
        let response = Response::from_string(LIVE_SCRIPT)
            .with_header(header("Content-Type", "text/javascript; charset=utf-8"))
            .with_header(header("Cache-Control", "no-store"));
        let _ = request.respond(response);
        return;
    }

    let output = Arc::clone(&shared.output.read().unwrap_or_else(|e| e.into_inner()));
    let (status, key, body) = match http::route(path, &output.files) {
        Route::Redirect(to) => {
            let response = Response::from_data(Vec::new())
                .with_status_code(301)
                .with_header(header("Location", &format!("{to}{query}")));
            let _ = request.respond(response);
            return;
        }
        Route::File(key) => match read(&output, &key) {
            Ok(body) => (200, key, body),
            Err(e) => {
                let _ = request.respond(text(500, &format!("cannot read {}: {e}", key.display())));
                return;
            }
        },
        Route::NotFound | Route::Rejected => {
            let key = std::path::PathBuf::from("404.html");
            match read(&output, &key) {
                Ok(body) => (404, key, body),
                Err(_) => {
                    let _ = request.respond(text(404, "404 not found"));
                    return;
                }
            }
        }
    };
    let ct = http::content_type(&key);
    let is_html = ct.starts_with("text/html");
    let body = if is_html {
        http::inject_live_script(&body)
    } else {
        body
    };
    let mut response = Response::from_data(body)
        .with_status_code(status)
        .with_header(header("Content-Type", ct));
    if is_html {
        response = response.with_header(header("Cache-Control", "no-store"));
    }
    let _ = request.respond(response);
}

fn read(output: &Output, key: &std::path::Path) -> io::Result<Vec<u8>> {
    match output.files.get(key) {
        Some(OutputFile::Bytes(b)) => Ok(b.clone()),
        Some(OutputFile::Copy(src)) => std::fs::read(src),
        None => Err(io::ErrorKind::NotFound.into()),
    }
}

/// Holds one browser's event stream open until it disconnects or the server stops.
fn stream_events(request: Request, shared: &Shared) {
    let (tx, rx) = mpsc::channel();
    shared
        .clients
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(tx);
    let mut w = request.into_writer();
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n: connected\n\n";
    if w.write_all_flush(head.as_bytes()).is_err() {
        return;
    }
    loop {
        let msg = match rx.recv_timeout(HEARTBEAT) {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) => PING,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        if w.write_all_flush(msg).is_err() {
            return;
        }
    }
}

trait WriteFlush {
    fn write_all_flush(&mut self, bytes: &[u8]) -> io::Result<()>;
}

impl<W: io::Write + ?Sized> WriteFlush for W {
    fn write_all_flush(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.write_all(bytes)?;
        self.flush()
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header is valid ASCII")
}

fn text(status: u16, body: &str) -> Response<io::Cursor<Vec<u8>>> {
    Response::from_string(body)
        .with_status_code(status)
        .with_header(header("Content-Type", "text/plain; charset=utf-8"))
}
