//! Your browser, lent to the agents of an Argus server: a Chrome of its own, started here and
//! carried over one WebSocket to Argus, which gives its agents a loopback address for it.
//!
//! Nothing listens on this computer and no SSH is involved: Chrome's debugging port is on loopback
//! here, and the only connection that leaves is ours, to Argus — the same road the sync takes. The
//! Chrome is started with a profile of its own (`browser-profile` in the config directory), so it
//! carries none of your other sessions, and Chrome itself refuses remote debugging on your normal
//! profile anyway.
//!
//! The bridge's frames are JSON text. Argus sends `open {ch, path}`, `msg {ch, data}`,
//! `close {ch}`, `bye`; this side sends `hello {browser, ws, client, os}`, `opened {ch}`,
//! `msg {ch, data}`, `close {ch, why}`. Each channel is one connection of an agent, carried to
//! Chrome's own `ws://127.0.0.1:<port><path>` by a thread of its own.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// What a link from Argus carries: `argus-sync://browser?url=…&key=…&name=…`.
#[derive(Debug, Clone, PartialEq)]
pub struct Lend {
    pub url: String,
    pub key: String,
    pub name: String,
}

pub fn parse_link(text: &str) -> Option<Lend> {
    let text = text.trim();
    if !text.contains("browser") && !text.contains("ab_") {
        return None;
    }
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
    let key = key.filter(|k| k.starts_with("ab_"))?;
    Some(Lend { url: url?.trim_end_matches('/').to_string(), key, name })
}

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
pub struct Started {
    pub child: Child,
    pub port: u16,
    /// `/devtools/browser/<id>`: the id is the secret Argus keeps from everybody but its agents.
    pub ws_path: String,
}

impl Drop for Started {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start Chrome with its own profile and a debugging port it picks, and wait for it to say which.
pub fn start_chrome(exe: &Path, profile: &Path, extra: &[String]) -> io::Result<Started> {
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
    let mut started = Started { child, port: 0, ws_path: String::new() };
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

/// How a bridge ended.
#[derive(Debug, PartialEq)]
pub enum Ended {
    /// Argus gave the browser back (Disconnect, or the key taken back): stop, do not reconnect.
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

/// Carry the browser to Argus until it is given back, `stop` is set, or the connection goes.
pub fn bridge(lend: &Lend, chrome: &Started, stop: &AtomicBool, say: &dyn Fn(&str)) -> Ended {
    let url = format!("{}/api/browser/bridge", lend.url.replacen("http", "ws", 1));
    let mut request = match tungstenite::client::IntoClientRequest::into_client_request(url.as_str()) {
        Ok(r) => r,
        Err(e) => return Ended::Lost(e.to_string()),
    };
    request.headers_mut().insert("authorization", format!("Bearer {}", lend.key).parse().unwrap());
    request
        .headers_mut()
        .insert("user-agent", concat!("argus-sync/", env!("CARGO_PKG_VERSION")).parse().unwrap());
    let mut server = match tungstenite::connect(request) {
        Ok((ws, _)) => ws,
        Err(tungstenite::Error::Http(r)) if r.status().as_u16() == 403 || r.status().as_u16() == 401 => {
            return Ended::GivenBack
        }
        Err(e) => return Ended::Lost(e.to_string()),
    };
    set_timeout(server.get_ref(), TICK);
    let hello = json!({"op": "hello", "browser": version(chrome.port), "ws": chrome.ws_path,
                       "client": concat!("argus-sync ", env!("CARGO_PKG_VERSION")), "os": std::env::consts::OS});
    if let Err(e) = server.send(Message::text(hello.to_string())) {
        return Ended::Lost(e.to_string());
    }
    say("lent: the agents on the server can drive this browser now");

    let (to_server, from_channels) = mpsc::channel::<Value>();
    let mut channels: HashMap<u64, (Sender<String>, Arc<AtomicBool>)> = HashMap::new();
    let mut pinged = Instant::now();
    let ended = loop {
        if stop.load(Ordering::Relaxed) {
            break Ended::Stopped;
        }
        match server.read() {
            Ok(Message::Text(text)) => {
                let frame: Value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                let ch = frame.get("ch").and_then(Value::as_u64).unwrap_or(0);
                match frame.get("op").and_then(Value::as_str) {
                    Some("open") => {
                        let path = frame.get("path").and_then(Value::as_str).unwrap_or("").to_string();
                        let (tx, rx) = mpsc::channel();
                        let gone = Arc::new(AtomicBool::new(false));
                        channels.insert(ch, (tx, gone.clone()));
                        let out = to_server.clone();
                        let port = chrome.port;
                        thread::spawn(move || channel(ch, port, &path, rx, out, gone));
                    }
                    Some("msg") => {
                        if let Some((tx, _)) = channels.get(&ch) {
                            let _ = tx.send(frame.get("data").and_then(Value::as_str).unwrap_or("").to_string());
                        }
                    }
                    Some("close") => {
                        if let Some((_, gone)) = channels.remove(&ch) {
                            gone.store(true, Ordering::Relaxed);
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
        // What the channels have for Argus.
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
                            return finish(channels, Ended::Lost(e.to_string()));
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
    let _ = server.close(None);
    let _ = server.flush();
    finish(channels, ended)
}

fn finish(channels: HashMap<u64, (Sender<String>, Arc<AtomicBool>)>, ended: Ended) -> Ended {
    for (_, (_, gone)) in channels {
        gone.store(true, Ordering::Relaxed);
    }
    ended
}

/// One agent's connection: Chrome's `ws://127.0.0.1:<port><path>`, both ways, until either side goes.
fn channel(ch: u64, port: u16, path: &str, inbox: Receiver<String>, out: Sender<Value>, gone: Arc<AtomicBool>) {
    let mut tab: WebSocket<MaybeTlsStream<TcpStream>> = match tungstenite::connect(format!("ws://127.0.0.1:{port}{path}")) {
        Ok((ws, _)) => ws,
        Err(e) => {
            let _ = out.send(json!({"op": "close", "ch": ch, "why": e.to_string()}));
            return;
        }
    };
    set_timeout(tab.get_ref(), TICK);
    let _ = out.send(json!({"op": "opened", "ch": ch}));
    let why = loop {
        if gone.load(Ordering::Relaxed) {
            let _ = tab.close(None);
            let _ = tab.flush();
            return;
        }
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
                Ok(data) => {
                    if let Err(e) = tab.send(Message::text(data)) {
                        if !would_block(&e) {
                            let _ = out.send(json!({"op": "close", "ch": ch, "why": e.to_string()}));
                            return;
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let _ = tab.close(None);
                    return;
                }
            }
        }
    };
    let _ = out.send(json!({"op": "close", "ch": ch, "why": why}));
}

/// Lend until given back or stopped: Chrome started once, the bridge made again when it drops.
pub fn lend(lend: &Lend, chrome: &Started, stop: &AtomicBool, say: &dyn Fn(&str)) -> Ended {
    let mut wait = Duration::from_secs(1);
    loop {
        match bridge(lend, chrome, stop, say) {
            Ended::Lost(why) => {
                say(&format!("connection to {} lost ({why}); trying again in {}s", lend.url, wait.as_secs()));
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

/// The link last used, kept so that `argus-sync browser` alone lends again (readable by you only).
pub fn saved(dir: &Path) -> Option<Lend> {
    fs::read_to_string(dir.join("browser-link")).ok().and_then(|t| parse_link(&t))
}

pub fn save(dir: &Path, link: &str) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join("browser-link");
    fs::write(&path, link.trim())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_from_argus_reads() {
        let lend = parse_link("argus-sync://browser?url=https%3A%2F%2Fgpu1.lab%3A8090%2F&key=ab_x-1&name=Mac").unwrap();
        assert_eq!(lend, Lend { url: "https://gpu1.lab:8090".into(), key: "ab_x-1".into(), name: "Mac".into() });
        assert!(parse_link("argus-sync://pair?url=https%3A%2F%2Fa&key=as_y&name=x").is_none(), "a sync link is not this");
    }
}
