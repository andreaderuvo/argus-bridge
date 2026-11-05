//! This computer, connected to an Argus server: its browser for the server's agents, a terminal on
//! it for you (in a window of Argus), a command an agent asks and you allow.
//!
//! One WebSocket, opened from here to Argus — the same road the sync takes. Nothing listens on this
//! computer and no SSH is involved. **What it lends is decided here**, by three switches, all off
//! until turned on: `browser` (a Chrome of its own, own profile: none of your sessions), `terminal`
//! (your shell, in a PTY), `actions` (one command at a time, as Argus asks after you said Do it).
//! A channel Argus opens for a switch that is off is refused here, whatever the server says.
//!
//! The bridge's frames are JSON text. Argus sends `open {ch, kind, …}` (kind `cdp {path}`, `pty
//! {cols, rows}`, `run {command, cwd, timeout}`), `msg {ch, data | b64}`, `resize {ch, cols, rows}`,
//! `close {ch}`, `bye`; this side sends `hello {name, os, client, offers, browser}`, `opened {ch}`,
//! `msg {ch, data | b64}`, `close {ch, why, code}`.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// What a link from Argus carries: `argus-sync://computer?url=…&key=…&name=…`.
#[derive(Debug, Clone, PartialEq)]
pub struct Invite {
    pub url: String,
    pub key: String,
    pub name: String,
}

pub fn parse_link(text: &str) -> Option<Invite> {
    let text = text.trim();
    let query = text.split_once('?').map(|(_, q)| q).unwrap_or(text);
    let (mut url, mut key, mut name) = (None, None, String::new());
    for part in query.split('&') {
        let (k, v) = part.split_once('=')?;
        let v = urlencoding::decode(v).ok()?.into_owned();
        match k {
            "url" => url = Some(v),
            "key" => key = Some(v),
            "name" => name = v,
            _ => {}
        }
    }
    let key = key.filter(|k| k.starts_with("ac_"))?;
    Some(Invite { url: url?.trim_end_matches('/').to_string(), key, name })
}

/// The three switches. Off until you turn them on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Offers {
    #[serde(default)]
    pub browser: bool,
    #[serde(default)]
    pub terminal: bool,
    #[serde(default)]
    pub actions: bool,
}

/// This computer's connection, kept in `computer.json` (readable by you only).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Setup {
    pub url: String,
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub offers: Offers,
    /// Connect when the app starts.
    #[serde(default)]
    pub on: bool,
}

impl Setup {
    pub fn load(dir: &Path) -> Option<Setup> {
        fs::read_to_string(dir.join("computer.json")).ok().and_then(|t| serde_json::from_str(&t).ok())
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let path = dir.join("computer.json");
        fs::write(&path, serde_json::to_string_pretty(self).unwrap_or_default())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn forget(dir: &Path) -> io::Result<()> {
        match fs::remove_file(dir.join("computer.json")) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

// ------------------------------------------------------------------------------------- the browser

/// Where Chrome is on this computer: `ARGUS_SYNC_CHROME` first, then the usual places.
pub fn find_chrome() -> Option<PathBuf> {
    if let Ok(said) = std::env::var("ARGUS_SYNC_CHROME") {
        let p = PathBuf::from(said);
        return p.is_file().then_some(p);
    }
    let mut places: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        for app in ["Google Chrome", "Chromium", "Microsoft Edge", "Brave Browser"] {
            let exe = format!("/Applications/{app}.app/Contents/MacOS/{app}");
            places.push(PathBuf::from(&exe));
            if let Some(home) = dirs::home_dir() {
                places.push(home.join(exe.trim_start_matches('/')));
            }
        }
    } else if cfg!(windows) {
        for var in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
            if let Ok(base) = std::env::var(var) {
                let base = PathBuf::from(base);
                places.push(base.join("Google/Chrome/Application/chrome.exe"));
                places.push(base.join("Microsoft/Edge/Application/msedge.exe"));
            }
        }
    } else {
        for name in ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge"] {
            if let Some(path) = std::env::var_os("PATH") {
                for dir in std::env::split_paths(&path) {
                    places.push(dir.join(name));
                }
            }
        }
    }
    places.into_iter().find(|p| p.is_file())
}

/// A Chrome started for lending, and how to reach it here.
pub struct Chrome {
    pub child: Child,
    pub port: u16,
    /// `/devtools/browser/<id>`: the id is the secret Argus keeps from everybody but its agents.
    pub ws_path: String,
}

impl Drop for Chrome {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start Chrome with its own profile and a debugging port it picks, and wait for it to say which.
pub fn start_chrome(exe: &Path, profile: &Path, extra: &[String]) -> io::Result<Chrome> {
    fs::create_dir_all(profile)?;
    let active = profile.join("DevToolsActivePort");
    let _ = fs::remove_file(&active);
    let child = Command::new(exe)
        .arg("--remote-debugging-port=0")
        .arg(format!("--user-data-dir={}", profile.display()))
        .args(["--no-first-run", "--no-default-browser-check"])
        .args(extra)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut started = Chrome { child, port: 0, ws_path: String::new() };
    let until = Instant::now() + Duration::from_secs(30);
    while Instant::now() < until {
        if let Some(status) = started.child.try_wait()? {
            return Err(io::Error::other(format!(
                "Chrome ended at once ({status}) — is another Chrome using {} already?",
                profile.display()
            )));
        }
        if let Ok(text) = fs::read_to_string(&active) {
            let mut lines = text.lines();
            if let (Some(port), Some(path)) = (lines.next(), lines.next()) {
                if let Ok(port) = port.trim().parse() {
                    started.port = port;
                    started.ws_path = path.trim().to_string();
                    return Ok(started);
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(io::Error::other("Chrome did not open its debugging port within 30 seconds"))
}

/// The browser this computer lends: started when the connection begins, and started again when an
/// agent needs it after you closed it — closing that window is not switching Browser off.
pub struct Browser {
    exe: PathBuf,
    profile: PathBuf,
    extra: Vec<String>,
    now: Mutex<Option<Chrome>>,
}

impl Browser {
    pub fn new(exe: PathBuf, profile: PathBuf, extra: Vec<String>) -> Browser {
        Browser { exe, profile, extra, now: Mutex::new(None) }
    }

    /// (port, `/devtools/browser/<id>`, started now): the Chrome running, or a new one if it was closed.
    pub fn ensure(&self) -> io::Result<(u16, String, bool)> {
        let mut now = self.now.lock().unwrap();
        if let Some(c) = now.as_mut() {
            if matches!(c.child.try_wait(), Ok(None)) {
                return Ok((c.port, c.ws_path.clone(), false));
            }
        }
        *now = None;
        let c = start_chrome(&self.exe, &self.profile, &self.extra)?;
        let out = (c.port, c.ws_path.clone(), true);
        *now = Some(c);
        Ok(out)
    }

    /// Close the Chrome it started (Disconnect, or the app quitting).
    pub fn stop(&self) {
        *self.now.lock().unwrap() = None;
    }
}

/// What Chrome calls itself ("Chrome/131.0…"), from its own /json/version on loopback.
pub fn version(port: u16) -> String {
    ureq::get(&format!("http://127.0.0.1:{port}/json/version"))
        .timeout(Duration::from_secs(5))
        .call()
        .ok()
        .and_then(|r| r.into_json::<Value>().ok())
        .and_then(|v| v.get("Browser").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| "Chrome".into())
}

// ------------------------------------------------------------------------------------- the bridge

/// How a bridge ended.
#[derive(Debug, PartialEq)]
pub enum Ended {
    /// Argus disconnected this computer, or forgot it: stop, do not reconnect.
    GivenBack,
    /// Asked to stop here.
    Stopped,
    /// The connection went: try again.
    Lost(String),
}

const TICK: Duration = Duration::from_millis(15);
const PING_EVERY: Duration = Duration::from_secs(20);

fn set_timeout(stream: &MaybeTlsStream<TcpStream>, d: Duration) {
    let tcp = match stream {
        MaybeTlsStream::Plain(s) => s,
        MaybeTlsStream::Rustls(s) => s.get_ref(),
        _ => return,
    };
    let _ = tcp.set_read_timeout(Some(d));
}

fn would_block(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(io) if matches!(io.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut))
}

type Out = Sender<Value>;

/// What a channel's thread is handed from Argus.
enum In {
    Frame(Value),
    Close,
}

fn hello(setup: &Setup, offers: Offers, browser: Option<(u16, String)>) -> Value {
    let mut hello = json!({"op": "hello", "name": setup.name, "os": std::env::consts::OS,
                           "client": concat!("argus-sync ", env!("CARGO_PKG_VERSION")), "offers": offers});
    if let (true, Some((port, ws))) = (offers.browser, browser) {
        hello["browser"] = json!({"ws": ws, "version": version(port)});
    }
    hello
}

/// Carry this computer to Argus until disconnected there, `stop` is set, or the connection goes.
pub fn bridge(setup: &Setup, browser: Option<&Arc<Browser>>, stop: &AtomicBool, say: &dyn Fn(&str)) -> Ended {
    let url = format!("{}/api/computers/bridge", setup.url.replacen("http", "ws", 1));
    let mut request = match tungstenite::client::IntoClientRequest::into_client_request(url.as_str()) {
        Ok(r) => r,
        Err(e) => return Ended::Lost(e.to_string()),
    };
    request.headers_mut().insert("authorization", format!("Bearer {}", setup.key).parse().unwrap());
    request
        .headers_mut()
        .insert("user-agent", concat!("argus-sync/", env!("CARGO_PKG_VERSION")).parse().unwrap());
    let mut server = match tungstenite::connect(request) {
        Ok((ws, _)) => ws,
        Err(tungstenite::Error::Http(r)) if matches!(r.status().as_u16(), 401 | 403) => {
            say("Argus does not know this computer any more (forgotten there): connect it again with a new link");
            return Ended::GivenBack;
        }
        Err(e) => return Ended::Lost(e.to_string()),
    };
    set_timeout(server.get_ref(), TICK);
    // The Chrome running now — or one started again, if it was closed since.
    let running = browser.and_then(|b| match b.ensure() {
        Ok((port, ws, _)) => Some((port, ws)),
        Err(e) => {
            say(&format!("the browser could not start: {e}"));
            None
        }
    });
    let offers = Offers { browser: setup.offers.browser && running.is_some(), ..setup.offers };
    if let Err(e) = server.send(Message::text(hello(setup, offers, running).to_string())) {
        return Ended::Lost(e.to_string());
    }
    say(&format!(
        "connected to {} — browser {}, terminal {}, actions {}",
        setup.url,
        if offers.browser { "on" } else { "off" },
        if offers.terminal { "on" } else { "off" },
        if offers.actions { "on" } else { "off" }
    ));

    let (to_server, from_channels) = mpsc::channel::<Value>();
    let mut channels: HashMap<u64, Sender<In>> = HashMap::new();
    // Ports of the server shown on this computer's localhost, and the connections made to them.
    let mut listening: HashMap<u64, Arc<AtomicBool>> = HashMap::new();
    let tcp: Tcp = Arc::new(Mutex::new(HashMap::new()));
    let mut pinged = Instant::now();
    let ended = 'outer: loop {
        if stop.load(Ordering::Relaxed) {
            break Ended::Stopped;
        }
        match server.read() {
            Ok(Message::Text(text)) => {
                let frame: Value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                let ch = frame.get("ch").and_then(Value::as_u64).unwrap_or(0);
                match frame.get("op").and_then(Value::as_str) {
                    Some("open") => {
                        let kind = frame.get("kind").and_then(Value::as_str).unwrap_or("");
                        let refuse = match kind {
                            "cdp" if !offers.browser => Some("the browser is off on this computer"),
                            "pty" if !offers.terminal => Some("the terminal is off on this computer"),
                            "run" if !offers.actions => Some("actions are off on this computer"),
                            "cdp" | "pty" | "run" => None,
                            _ => Some("not something this computer does"),
                        };
                        if let Some(why) = refuse {
                            let _ = to_server.send(json!({"op": "close", "ch": ch, "why": why}));
                            continue;
                        }
                        let (tx, rx) = mpsc::channel();
                        channels.insert(ch, tx);
                        let out = to_server.clone();
                        let frame = frame.clone();
                        let lent = browser.cloned();
                        let again = setup.clone();
                        thread::spawn(move || match frame.get("kind").and_then(Value::as_str) {
                            Some("cdp") => {
                                // Closed since? Started again, and Argus told its new address.
                                let Some(b) = lent else { return };
                                match b.ensure() {
                                    Ok((port, ws, restarted)) => {
                                        if restarted {
                                            let _ = out.send(hello(&again, offers, Some((port, ws.clone()))));
                                        }
                                        let asked = frame.get("path").and_then(Value::as_str).unwrap_or("");
                                        let path = if asked.starts_with("/devtools/browser/") { ws.as_str() } else { asked };
                                        cdp(ch, port, path, rx, out)
                                    }
                                    Err(e) => {
                                        let _ = out.send(json!({"op": "close", "ch": ch, "why": format!("the browser did not start again: {e}")}));
                                    }
                                }
                            }
                            Some("pty") => terminal(ch, &frame, rx, out),
                            _ => run(ch, &frame, rx, out),
                        });
                    }
                    Some("msg") | Some("resize") => {
                        if let Some(tx) = channels.get(&ch) {
                            let _ = tx.send(In::Frame(frame));
                        }
                    }
                    Some("close") => {
                        if let Some(tx) = channels.remove(&ch) {
                            let _ = tx.send(In::Close);
                        }
                    }
                    Some("forward") => {
                        let port = frame.get("port").and_then(Value::as_u64).unwrap_or(0);
                        let local = frame.get("local").and_then(Value::as_u64).unwrap_or(port);
                        if let Some(old) = listening.remove(&port) {
                            old.store(true, Ordering::Relaxed);
                        }
                        match listen_here(local as u16) {
                            Ok((listener, at)) => {
                                let stop = Arc::new(AtomicBool::new(false));
                                listening.insert(port, stop.clone());
                                let (tcp, out) = (tcp.clone(), to_server.clone());
                                thread::spawn(move || accept(port, listener, stop, tcp, out));
                                let _ = to_server.send(json!({"op": "forwarded", "port": port, "local": at, "ok": true}));
                            }
                            Err(e) => {
                                let _ = to_server.send(json!({"op": "forwarded", "port": port, "ok": false,
                                                              "why": format!("nothing could listen on localhost:{local} here: {e}")}));
                            }
                        }
                    }
                    Some("unforward") => {
                        if let Some(old) = listening.remove(&frame.get("port").and_then(Value::as_u64).unwrap_or(0)) {
                            old.store(true, Ordering::Relaxed);
                        }
                    }
                    Some("tcp-opened") | Some("tcp-data") | Some("tcp-close") => {
                        let key = frame.get("ch").and_then(Value::as_str).unwrap_or("").to_string();
                        let mut open = tcp.lock().unwrap();
                        if frame.get("op").and_then(Value::as_str) == Some("tcp-close") {
                            if let Some(tx) = open.remove(&key) {
                                let _ = tx.send(In::Close);
                            }
                        } else if let Some(tx) = open.get(&key) {
                            let _ = tx.send(In::Frame(frame.clone()));
                        }
                    }
                    Some("bye") => break Ended::GivenBack,
                    _ => {}
                }
            }
            Ok(Message::Close(frame)) => {
                let why = frame.map(|f| f.reason.to_string()).unwrap_or_default();
                break if why.contains("given back") || why.contains("taken back") { Ended::GivenBack } else { Ended::Lost(why) };
            }
            Ok(_) => {}
            Err(e) if would_block(&e) => {}
            Err(e) => break Ended::Lost(e.to_string()),
        }
        loop {
            match from_channels.try_recv() {
                Ok(frame) => {
                    if frame.get("op").and_then(Value::as_str) == Some("close") {
                        if let Some(ch) = frame.get("ch").and_then(Value::as_u64) {
                            channels.remove(&ch);
                        }
                    }
                    if let Err(e) = server.send(Message::text(frame.to_string())) {
                        if !would_block(&e) {
                            break 'outer Ended::Lost(e.to_string());
                        }
                    }
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        if pinged.elapsed() > PING_EVERY {
            pinged = Instant::now();
            let _ = server.send(Message::Ping(Vec::new().into()));
        }
    };
    for (_, tx) in channels {
        let _ = tx.send(In::Close);
    }
    for (_, stop) in listening {
        stop.store(true, Ordering::Relaxed);
    }
    for (_, tx) in tcp.lock().unwrap().drain() {
        let _ = tx.send(In::Close);
    }
    let _ = server.close(None);
    let _ = server.flush();
    ended
}

/// Connect until disconnected in Argus or stopped here: made again when the connection drops.
pub fn keep(setup: &Setup, browser: Option<&Arc<Browser>>, stop: &AtomicBool, say: &dyn Fn(&str)) -> Ended {
    let mut wait = Duration::from_secs(1);
    loop {
        match bridge(setup, browser, stop, say) {
            Ended::Lost(why) => {
                say(&format!("connection to {} lost ({why}); trying again in {}s", setup.url, wait.as_secs()));
                let until = Instant::now() + wait;
                while Instant::now() < until {
                    if stop.load(Ordering::Relaxed) {
                        return Ended::Stopped;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                wait = (wait * 2).min(Duration::from_secs(30));
            }
            other => return other,
        }
    }
}

// ------------------------------------------------------------------------------------- ports shown here

type Tcp = Arc<Mutex<HashMap<String, Sender<In>>>>;

/// A listener on this computer's loopback: the port asked, or the next free one of twenty.
fn listen_here(from: u16) -> io::Result<(std::net::TcpListener, u16)> {
    let mut last = None;
    for at in from..from.saturating_add(20) {
        match std::net::TcpListener::bind(("127.0.0.1", at)) {
            Ok(l) => {
                l.set_nonblocking(true)?;
                return Ok((l, at));
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("no port free")))
}

/// Every connection made to localhost:<local> here, carried to the port on the server.
fn accept(port: u64, listener: std::net::TcpListener, stop: Arc<AtomicBool>, tcp: Tcp, out: Out) {
    let mut n = 0u64;
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((sock, _)) => {
                n += 1;
                let ch = format!("c{port}-{n}");
                let (tx, rx) = mpsc::channel();
                tcp.lock().unwrap().insert(ch.clone(), tx);
                let (out, tcp) = (out.clone(), tcp.clone());
                thread::spawn(move || {
                    pump(&ch, port, sock, rx, &out);
                    tcp.lock().unwrap().remove(&ch);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(50)),
            Err(_) => thread::sleep(Duration::from_millis(200)),
        }
    }
}

fn pump(ch: &str, port: u64, mut sock: TcpStream, inbox: Receiver<In>, out: &Out) {
    let _ = out.send(json!({"op": "tcp", "ch": ch, "port": port}));
    match inbox.recv_timeout(Duration::from_secs(15)) {
        Ok(In::Frame(f)) if f.get("op").and_then(Value::as_str) == Some("tcp-opened") => {}
        _ => return,
    }
    let _ = sock.set_nonblocking(false);
    let _ = sock.set_read_timeout(Some(TICK));
    let mut buf = [0u8; 65536];
    loop {
        match sock.read(&mut buf) {
            Ok(0) => {
                let _ = out.send(json!({"op": "tcp-close", "ch": ch}));
                return;
            }
            Ok(n) => {
                let _ = out.send(json!({"op": "tcp-data", "ch": ch, "b64": B64.encode(&buf[..n])}));
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(_) => {
                let _ = out.send(json!({"op": "tcp-close", "ch": ch}));
                return;
            }
        }
        loop {
            match inbox.try_recv() {
                Ok(In::Frame(f)) => {
                    if let Some(bytes) = f.get("b64").and_then(Value::as_str).and_then(|b| B64.decode(b).ok()) {
                        if sock.write_all(&bytes).is_err() {
                            let _ = out.send(json!({"op": "tcp-close", "ch": ch}));
                            return;
                        }
                    }
                }
                Ok(In::Close) | Err(TryRecvError::Disconnected) => {
                    let _ = sock.shutdown(std::net::Shutdown::Both);
                    return;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
    }
}

// ------------------------------------------------------------------------------------- the channels

/// An agent's connection to the browser: Chrome's `ws://127.0.0.1:<port><path>`, both ways.
fn cdp(ch: u64, port: u16, path: &str, inbox: Receiver<In>, out: Out) {
    let mut tab: WebSocket<MaybeTlsStream<TcpStream>> = match tungstenite::connect(format!("ws://127.0.0.1:{port}{path}")) {
        Ok((ws, _)) => ws,
        Err(e) => {
            let _ = out.send(json!({"op": "close", "ch": ch, "why": e.to_string()}));
            return;
        }
    };
    set_timeout(tab.get_ref(), TICK);
    let _ = out.send(json!({"op": "opened", "ch": ch}));
    let why = 'outer: loop {
        match tab.read() {
            Ok(Message::Text(text)) => {
                let _ = out.send(json!({"op": "msg", "ch": ch, "data": text.as_str()}));
            }
            Ok(Message::Binary(bytes)) => {
                let _ = out.send(json!({"op": "msg", "ch": ch, "data": String::from_utf8_lossy(&bytes)}));
            }
            Ok(Message::Close(_)) => break "Chrome closed it".to_string(),
            Ok(_) => {}
            Err(e) if would_block(&e) => {}
            Err(e) => break e.to_string(),
        }
        loop {
            match inbox.try_recv() {
                Ok(In::Frame(f)) => {
                    let data = f.get("data").and_then(Value::as_str).unwrap_or("").to_string();
                    if let Err(e) = tab.send(Message::text(data)) {
                        if !would_block(&e) {
                            break 'outer e.to_string();
                        }
                    }
                }
                Ok(In::Close) | Err(TryRecvError::Disconnected) => {
                    let _ = tab.close(None);
                    let _ = tab.flush();
                    return;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
    };
    let _ = out.send(json!({"op": "close", "ch": ch, "why": why}));
}

/// Your shell, in a PTY: what you type in Argus's window, and what it prints.
fn terminal(ch: u64, open: &Value, inbox: Receiver<In>, out: Out) {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    let size = |f: &Value| PtySize {
        rows: f.get("rows").and_then(Value::as_u64).unwrap_or(24).clamp(2, 1000) as u16,
        cols: f.get("cols").and_then(Value::as_u64).unwrap_or(80).clamp(2, 1000) as u16,
        pixel_width: 0,
        pixel_height: 0,
    };
    let fail = |why: String| {
        let _ = out.send(json!({"op": "close", "ch": ch, "why": why}));
    };
    let pair = match native_pty_system().openpty(size(open)) {
        Ok(p) => p,
        Err(e) => return fail(format!("no terminal here: {e}")),
    };
    let mut cmd = if cfg!(windows) {
        CommandBuilder::new(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()))
    } else {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| if cfg!(target_os = "macos") { "/bin/zsh".into() } else { "/bin/sh".into() });
        let mut c = CommandBuilder::new(shell);
        c.arg("-l");
        c
    };
    if let Some(home) = dirs::home_dir() {
        cmd.cwd(home);
    }
    cmd.env("TERM", "xterm-256color");
    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => return fail(format!("the shell did not start: {e}")),
    };
    drop(pair.slave);
    let mut reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => return fail(e.to_string()),
    };
    let mut writer = match pair.master.take_writer() {
        Ok(w) => w,
        Err(e) => return fail(e.to_string()),
    };
    let _ = out.send(json!({"op": "opened", "ch": ch}));
    let ended = Arc::new(AtomicBool::new(false));
    {
        let out = out.clone();
        let ended = ended.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let _ = out.send(json!({"op": "msg", "ch": ch, "b64": B64.encode(&buf[..n])}));
                    }
                }
            }
            ended.store(true, Ordering::Relaxed);
        });
    }
    loop {
        if ended.load(Ordering::Relaxed) {
            let code = child.wait().ok().map(|s| s.exit_code());
            let _ = out.send(json!({"op": "close", "ch": ch, "why": "the shell ended", "code": code}));
            return;
        }
        match inbox.recv_timeout(Duration::from_millis(50)) {
            Ok(In::Frame(f)) => match f.get("op").and_then(Value::as_str) {
                Some("resize") => {
                    let _ = pair.master.resize(size(&f));
                }
                _ => {
                    if let Some(bytes) = f.get("b64").and_then(Value::as_str).and_then(|b| B64.decode(b).ok()) {
                        let _ = writer.write_all(&bytes);
                        let _ = writer.flush();
                    }
                }
            },
            Ok(In::Close) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = child.kill();
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

/// One command, as Argus asked after you allowed it: its output as it comes, then its exit code.
fn run(ch: u64, open: &Value, inbox: Receiver<In>, out: Out) {
    let command = open.get("command").and_then(Value::as_str).unwrap_or("");
    let cwd = open.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()).map(PathBuf::from).or_else(dirs::home_dir);
    let timeout = Duration::from_secs_f64(open.get("timeout").and_then(Value::as_f64).unwrap_or(120.0).clamp(1.0, 600.0));
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new(std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()));
        c.args(["-lc", command]);
        c
    };
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = match cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = out.send(json!({"op": "close", "ch": ch, "why": format!("it did not start: {e}")}));
            return;
        }
    };
    let _ = out.send(json!({"op": "opened", "ch": ch}));
    let pipes: Vec<Box<dyn Read + Send>> = vec![
        Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
        Box::new(child.stderr.take().unwrap()),
    ];
    let readers: Vec<_> = pipes
        .into_iter()
        .map(|mut pipe| {
            let out = out.clone();
            thread::spawn(move || {
                let mut buf = [0u8; 8192];
                while let Ok(n) = pipe.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let _ = out.send(json!({"op": "msg", "ch": ch, "data": String::from_utf8_lossy(&buf[..n])}));
                }
            })
        })
        .collect();
    let started = Instant::now();
    let code = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break status.code();
        }
        if started.elapsed() > timeout || matches!(inbox.try_recv(), Ok(In::Close)) {
            let _ = child.kill();
            let _ = child.wait();
            for r in readers {
                let _ = r.join();
            }
            let _ = out.send(json!({"op": "close", "ch": ch, "why": "stopped: it took too long", "code": Value::Null}));
            return;
        }
        thread::sleep(Duration::from_millis(30));
    };
    for r in readers {
        let _ = r.join();
    }
    let _ = out.send(json!({"op": "close", "ch": ch, "code": code}));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_from_argus_reads() {
        let invite = parse_link("argus-sync://computer?url=https%3A%2F%2Fgpu1.lab%3A8090%2F&key=ac_x-1&name=Mac").unwrap();
        assert_eq!(invite, Invite { url: "https://gpu1.lab:8090".into(), key: "ac_x-1".into(), name: "Mac".into() });
        assert!(parse_link("argus-sync://pair?url=https%3A%2F%2Fa&key=as_y&name=x").is_none(), "a sync link is not this");
    }

    #[test]
    fn the_switches_start_off_and_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Setup::load(dir.path()).is_none());
        let mut setup = Setup { url: "https://a".into(), key: "ac_k".into(), name: "Mac".into(), offers: Offers::default(), on: false };
        assert_eq!(setup.offers, Offers { browser: false, terminal: false, actions: false });
        setup.offers.terminal = true;
        setup.save(dir.path()).unwrap();
        assert_eq!(Setup::load(dir.path()).unwrap(), setup);
        Setup::forget(dir.path()).unwrap();
        assert!(Setup::load(dir.path()).is_none());
    }
}
