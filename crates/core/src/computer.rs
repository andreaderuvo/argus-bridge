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

/// What a link from Argus carries (`argus-sync://` for the links made before the rename): `argus-bridge://computer?url=…&key=…&name=…`.
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
    /// Which of the links (`Links`) this is; empty in the `computer.json` of before.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub url: String,
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub offers: Offers,
    /// Connect when the app starts.
    #[serde(default)]
    pub on: bool,
    /// How the lent browser is started: the person's to decide (the app's This computer → Browser).
    #[serde(default)]
    pub browser: BrowserSettings,
}

/// How the lent browser starts. Kept with the computer's connection; used the next time it starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrowserSettings {
    /// Not said to every page that a debugging port drives it (navigator.webdriver). On by default: said,
    /// Google refuses to sign in "a browser that may not be secure" (Argus issue #16).
    #[serde(default = "yes")]
    pub hide_automation: bool,
    /// Which browser: a path to Chrome, Chromium, Edge or Brave; empty = the first found.
    #[serde(default)]
    pub exe: String,
    /// More switches for it, separated by spaces — for whoever knows what they do.
    #[serde(default)]
    pub flags: String,
}

fn yes() -> bool {
    true
}

impl Default for BrowserSettings {
    fn default() -> Self {
        BrowserSettings { hide_automation: true, exe: String::new(), flags: String::new() }
    }
}

impl BrowserSettings {
    /// The switches it adds to the ones the bridge always gives (port, profile, no first-run).
    pub fn args(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.hide_automation {
            out.push("--disable-blink-features=AutomationControlled".to_string());
        }
        out.extend(self.flags.split_whitespace().map(str::to_string));
        out
    }

    /// The browser chosen, when it is there; else the first found here.
    pub fn exe(&self) -> Option<PathBuf> {
        let chosen = PathBuf::from(self.exe.trim());
        if !self.exe.trim().is_empty() && chosen.is_file() {
            return Some(chosen);
        }
        find_chrome()
    }
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

/// Every Argus this computer is connected to ("I must be able to manage several Argus", 2026-11-19): two VMs,
/// each its own server, each with its own switches. Kept in `computers.json` (readable by you only), with the
/// one browser they all borrow and how it starts. The single `computer.json` of 0.1.29 and before is taken
/// in at the first load, then removed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Links {
    #[serde(default)]
    pub links: Vec<Setup>,
    /// How the lent browser starts: one browser, lent to every Argus that has Browser on.
    #[serde(default)]
    pub browser: BrowserSettings,
}

impl Links {
    pub fn load(dir: &Path) -> Links {
        if let Some(links) = fs::read_to_string(dir.join("computers.json")).ok().and_then(|t| serde_json::from_str::<Links>(&t).ok()) {
            return links.numbered();
        }
        let Some(mut one) = Setup::load(dir) else { return Links::default() };
        let browser = std::mem::take(&mut one.browser);
        let links = Links { links: vec![one], browser }.numbered();
        if links.save(dir).is_ok() {
            let _ = Setup::forget(dir);
        }
        links
    }

    /// Every link with an id of its own: the app speaks of a link by it.
    fn numbered(mut self) -> Links {
        for i in 0..self.links.len() {
            if self.links[i].id.is_empty() || self.links[..i].iter().any(|l| l.id == self.links[i].id) {
                self.links[i].id = self.free_id();
            }
        }
        self
    }

    fn free_id(&self) -> String {
        (1..).map(|n: u32| n.to_string()).find(|id| !self.links.iter().any(|l| &l.id == id)).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let path = dir.join("computers.json");
        let part = dir.join("computers.json.part");
        fs::write(&part, serde_json::to_string_pretty(self).unwrap_or_default())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&part, fs::Permissions::from_mode(0o600))?;
        }
        fs::rename(&part, &path)
    }

    pub fn get(&self, id: &str) -> Option<&Setup> {
        self.links.iter().find(|l| l.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Setup> {
        self.links.iter_mut().find(|l| l.id == id)
    }

    /// A link from Argus taken in: the same server linked again replaces its old link (a new key, the
    /// switches chosen now) and keeps its id; another server is added. Returns the link's id.
    pub fn add(&mut self, mut setup: Setup) -> String {
        let same = |a: &str, b: &str| a.trim_end_matches('/') == b.trim_end_matches('/');
        if let Some(old) = self.links.iter_mut().find(|l| same(&l.url, &setup.url)) {
            setup.id = old.id.clone();
            *old = setup;
            return old.id.clone();
        }
        setup.id = self.free_id();
        let id = setup.id.clone();
        self.links.push(setup);
        id
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.links.len();
        self.links.retain(|l| l.id != id);
        self.links.len() != before
    }

    /// Whether any Argus connected here borrows the browser.
    pub fn lends_browser(&self) -> bool {
        self.links.iter().any(|l| l.on && l.offers.browser)
    }
}

/// A key for this computer's own Argus window ("I open Argus from argus-bridge and do not know the token",
/// 2026-11-23): asked of that Argus with the computer's key each time the window is made, and never written here —
/// a new one revokes the one before. Err when that Argus says no (its switch off, an older Argus) or cannot be
/// reached: the window then opens on the sign-in, as before.
pub fn window_key(setup: &Setup) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .user_agent(concat!("argus-bridge/", env!("CARGO_PKG_VERSION")))
        .build();
    let said: Value = agent
        .post(&format!("{}/api/computers/window-key", setup.url.trim_end_matches('/')))
        .set("authorization", &format!("Bearer {}", setup.key))
        .send_json(json!({}))
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())?;
    said.get("key").and_then(Value::as_str).filter(|k| !k.is_empty()).map(str::to_string).ok_or_else(|| "no key in the answer".to_string())
}

// ------------------------------------------------------------------------------------- the browser

/// Every browser found here that can be lent (Chrome, Chromium, Edge, Brave), for the person to choose.
pub fn browsers_here() -> Vec<PathBuf> {
    chrome_places().into_iter().filter(|p| p.is_file()).fold(Vec::new(), |mut out, p| {
        if !out.contains(&p) {
            out.push(p);
        }
        out
    })
}

/// Where Chrome is on this computer: `ARGUS_SYNC_CHROME` first, then the usual places.
pub fn find_chrome() -> Option<PathBuf> {
    if let Ok(said) = std::env::var("ARGUS_SYNC_CHROME") {
        let p = PathBuf::from(said);
        return p.is_file().then_some(p);
    }
    chrome_places().into_iter().find(|p| p.is_file())
}

/// The usual places of a browser that can be lent, best first.
fn chrome_places() -> Vec<PathBuf> {
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
                places.push(base.join("BraveSoftware/Brave-Browser/Application/brave.exe"));
            }
        }
    } else {
        for name in ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge", "brave-browser"] {
            if let Some(path) = std::env::var_os("PATH") {
                for dir in std::env::split_paths(&path) {
                    places.push(dir.join(name));
                }
            }
        }
    }
    places
}

/// A Chrome started for lending, and how to reach it here.
pub struct Chrome {
    /// The Chrome started here; `None` for one found already running on this profile and taken over.
    pub child: Option<Child>,
    /// The process of a Chrome taken over, ended with it — only while the profile's lock still names it.
    pub pid: Option<u32>,
    pub profile: PathBuf,
    pub port: u16,
    /// `/devtools/browser/<id>`: the id is the secret Argus keeps from everybody but its agents.
    pub ws_path: String,
}

impl Chrome {
    pub fn alive(&mut self) -> bool {
        match self.child.as_mut() {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => port_answers(self.port),
        }
    }
}

impl Drop for Chrome {
    fn drop(&mut self) {
        if let Some(c) = self.child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        } else if let Some(pid) = self.pid {
            if holder(&self.profile) == Some(pid) {
                end_process(pid);
            }
        }
    }
}

fn port_answers(port: u16) -> bool {
    port != 0 && std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(500)).is_ok()
}

fn read_active(profile: &Path) -> Option<(u16, String)> {
    let text = fs::read_to_string(profile.join("DevToolsActivePort")).ok()?;
    let mut lines = text.lines();
    let port = lines.next()?.trim().parse().ok()?;
    let path = lines.next()?.trim().to_string();
    path.starts_with("/devtools/browser/").then_some((port, path))
}

/// The Chrome holding this profile, from the lock it leaves in it ("<host>-<pid>"), if it is running.
fn holder(profile: &Path) -> Option<u32> {
    let target = fs::read_link(profile.join("SingletonLock")).ok()?;
    let pid: u32 = target.to_string_lossy().rsplit('-').next()?.parse().ok()?;
    process_alive(pid).then_some(pid)
}

/// How a process was started, as one line: None where it cannot be read.
fn command_line(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        Some(String::from_utf8_lossy(&raw).replace('\0', " "))
    }
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("ps").args(["-ww", "-o", "command=", "-p", &pid.to_string()]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Ends a Chrome of this profile: asked first, then made to.
fn end_process(pid: u32) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-TERM", &pid.to_string()]).stderr(Stdio::null()).status();
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until && process_alive(pid) {
            thread::sleep(Duration::from_millis(100));
        }
        if process_alive(pid) {
            let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).stderr(Stdio::null()).status();
            thread::sleep(Duration::from_millis(300));
        }
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
}

/// Start Chrome with its own profile and a debugging port it picks, and wait for it to say which.
///
/// The profile is this app's alone, so a Chrome already on it is one of ours that outlived the run that
/// started it (2026-11-09: the bridge restarted, its Chrome did not; the new one handed over to it and
/// ended at once, and the browser was refused until somebody quit Chrome by hand). It is taken over when
/// its debugging port answers, and ended — then started again — when it does not.
pub fn start_chrome(exe: &Path, profile: &Path, extra: &[String]) -> io::Result<Chrome> {
    fs::create_dir_all(profile)?;
    if let Some((port, ws_path)) = read_active(profile) {
        if port_answers(port) && !version(port).is_empty() {
            // Taken over only when it runs with the switches asked for now: 0.1.12 took over the Chrome
            // 0.1.11 had started, without the switch that hides automation, and Google went on refusing
            // to sign it in (2026-11-10). Otherwise it is ended, and one started as asked.
            let pid = holder(profile);
            let started_as_asked = pid.and_then(command_line).map(|line| extra.iter().all(|a| line.contains(a.as_str())));
            if started_as_asked != Some(false) {
                return Ok(Chrome { child: None, pid, profile: profile.to_path_buf(), port, ws_path });
            }
            if let Some(pid) = pid {
                end_process(pid);
                for lock in ["SingletonLock", "SingletonCookie", "SingletonSocket"] {
                    let _ = fs::remove_file(profile.join(lock));
                }
            }
        }
    }
    for attempt in 0..2 {
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
        let mut started = Chrome { child: Some(child), pid: None, profile: profile.to_path_buf(), port: 0, ws_path: String::new() };
        let until = Instant::now() + Duration::from_secs(30);
        let mut ended = None;
        while Instant::now() < until {
            if let Some(status) = started.child.as_mut().unwrap().try_wait()? {
                ended = Some(status);
                break;
            }
            if let Some((port, path)) = read_active(profile) {
                started.port = port;
                started.ws_path = path;
                return Ok(started);
            }
            thread::sleep(Duration::from_millis(100));
        }
        let Some(status) = ended else {
            return Err(io::Error::other("Chrome did not open its debugging port within 30 seconds"));
        };
        // Handed over to a Chrome already on this profile: that one is ended, and this tried again.
        match holder(profile) {
            Some(pid) if attempt == 0 => {
                end_process(pid);
                for lock in ["SingletonLock", "SingletonCookie", "SingletonSocket"] {
                    let _ = fs::remove_file(profile.join(lock));
                }
            }
            Some(_) => return Err(io::Error::other("another Chrome holds this computer's browser profile and would not end")),
            None => return Err(io::Error::other(format!("Chrome ended at once ({status})"))),
        }
    }
    Err(io::Error::other("Chrome could not be started"))
}

/// The browser this computer lends: started when the connection begins, and started again when an
/// agent needs it after you closed it — closing that window is not switching Browser off.
pub struct Browser {
    exe: PathBuf,
    profile: PathBuf,
    extra: Vec<String>,
    now: Mutex<Option<Chrome>>,
    /// Agents connected to it now, and when one last did something: what the app shows the person ("an agent
    /// is using your browser", 2026-11-12 — ChatGPT and Claude say so; this browser said nothing).
    driving: std::sync::atomic::AtomicUsize,
    last: Mutex<Option<Instant>>,
    /// Stopped by the person: every agent's connection closed, none let in again.
    cut: AtomicBool,
}

/// One agent's connection to the browser, counted while it lasts.
struct Driving<'a>(&'a Browser);

impl Drop for Driving<'_> {
    fn drop(&mut self) {
        self.0.driving.fetch_sub(1, Ordering::Relaxed);
        self.0.touch();
    }
}

impl Browser {
    pub fn new(exe: PathBuf, profile: PathBuf, extra: Vec<String>) -> Browser {
        Browser { exe, profile, extra, now: Mutex::new(None), driving: Default::default(), last: Mutex::new(None), cut: AtomicBool::new(false) }
    }

    fn touch(&self) {
        *self.last.lock().unwrap() = Some(Instant::now());
    }

    fn drive(&self) -> Driving<'_> {
        self.driving.fetch_add(1, Ordering::Relaxed);
        self.touch();
        Driving(self)
    }

    /// (agents connected now, how long since one last did something): an agent's tools connect for each step,
    /// so "in use" is either, the second within a short while.
    pub fn agents(&self) -> (usize, Option<Duration>) {
        (self.driving.load(Ordering::Relaxed), self.last.lock().unwrap().map(|t| t.elapsed()))
    }

    /// Stop every agent in this browser now, and let none in again (until the browser is lent anew).
    pub fn stop_agents(&self) {
        self.cut.store(true, Ordering::Relaxed);
    }

    pub fn stopped(&self) -> bool {
        self.cut.load(Ordering::Relaxed)
    }

    /// (port, `/devtools/browser/<id>`, started now): the Chrome running, or a new one if it was closed.
    pub fn ensure(&self) -> io::Result<(u16, String, bool)> {
        let mut now = self.now.lock().unwrap();
        if let Some(c) = now.as_mut() {
            if c.alive() {
                return Ok((c.port, c.ws_path.clone(), false));
            }
        }
        *now = None;
        let c = start_chrome(&self.exe, &self.profile, &self.extra)?;
        let out = (c.port, c.ws_path.clone(), true);
        *now = Some(c);
        Ok(out)
    }

    /// The Chrome running now, if any — without starting one.
    pub fn running(&self) -> Option<(u16, String)> {
        let mut now = self.now.lock().unwrap();
        let c = now.as_mut()?;
        c.alive().then(|| (c.port, c.ws_path.clone()))
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

pub(crate) type Out = Sender<Value>;

/// What a channel's thread is handed from Argus.
pub(crate) enum In {
    Frame(Value),
    Close,
}

fn hello(setup: &Setup, offers: Offers, browser: Option<(u16, String)>) -> Value {
    let mut hello = json!({"op": "hello", "name": setup.name, "os": std::env::consts::OS,
                           "client": concat!("argus-bridge ", env!("CARGO_PKG_VERSION")), "offers": offers});
    if let (true, Some((port, _))) = (offers.browser, browser) {
        hello["browser"] = json!({"version": version(port)});
    }
    hello
}

/// A bell rung in Argus — an agent finished, failed, or asks — for this computer to show as a
/// notification of its own.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Told {
    pub why: String,
    pub session: String,
    pub text: String,
    /// A question of Argus (`asks`) this bell is: its id and the answers offered ("Do it", "No").
    pub ask: String,
    pub options: Vec<String>,
}

/// How a question is answered from here: one of its `options`, sent back to Argus.
pub type Answer = Box<dyn FnOnce(String) + Send>;

/// Shows a bell; given an `Answer` when it is a question that can be answered from the notification.
pub type Bell<'a> = &'a dyn Fn(&Told, Option<Answer>);

/// A `bell` frame read: what it says, and the question it carries if any.
pub fn told(frame: &Value) -> Told {
    let get = |k: &str| frame.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let options = frame.get("options").and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    Told { why: get("why"), session: get("session"), text: get("text"), ask: get("ask"), options }
}

/// Carry this computer to Argus until disconnected there, `stop` is set, or the connection goes.
pub fn bridge(setup: &Setup, browser: Option<&Arc<Browser>>, logins: Option<&crate::logins::Logins>, stop: &AtomicBool, say: &dyn Fn(&str), bell: Bell) -> Ended {
    let url = format!("{}/api/computers/bridge", setup.url.replacen("http", "ws", 1));
    let mut request = match tungstenite::client::IntoClientRequest::into_client_request(url.as_str()) {
        Ok(r) => r,
        Err(e) => return Ended::Lost(e.to_string()),
    };
    request.headers_mut().insert("authorization", format!("Bearer {}", setup.key).parse().unwrap());
    request
        .headers_mut()
        .insert("user-agent", concat!("argus-bridge/", env!("CARGO_PKG_VERSION")).parse().unwrap());
    let mut server = match tungstenite::connect(request) {
        Ok((ws, _)) => ws,
        Err(tungstenite::Error::Http(r)) if matches!(r.status().as_u16(), 401 | 403) => {
            say("Argus does not know this computer any more (forgotten there): connect it again with a new link");
            return Ended::GivenBack;
        }
        Err(e) => return Ended::Lost(e.to_string()),
    };
    set_timeout(server.get_ref(), TICK);
    // The browser is offered, not started: Chrome opens only when an agent first uses it (a window
    // popping up the moment this connects was asked away, 2026-11-05) — and Argus's address for it
    // does not change with each Chrome.
    let running = browser.and_then(|b| b.running());
    let offers = Offers { browser: setup.offers.browser && browser.is_some(), ..setup.offers };
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
                            // Files read and written here: the same power as a command run here, under the same switch.
                            "file" if !offers.actions => Some("actions are off on this computer"),
                            "login" if !offers.browser => Some("the browser is off on this computer"),
                            "login" if logins.is_none() => Some("signing agents in is not offered by this client"),
                            "cdp" | "pty" | "run" | "login" | "file" => None,
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
                        let logins = logins.cloned();
                        let again = setup.clone();
                        thread::spawn(move || match frame.get("kind").and_then(Value::as_str) {
                            Some("cdp") => {
                                // Closed since? Started again, and Argus told its new address.
                                let Some(b) = lent else { return };
                                if b.stopped() {
                                    let _ = out.send(json!({"op": "close", "ch": ch, "why": "the person stopped agents in this browser — they lend it again from argus-bridge"}));
                                    return;
                                }
                                match b.ensure() {
                                    Ok((port, ws, restarted)) => {
                                        if restarted {
                                            let _ = out.send(hello(&again, offers, Some((port, ws.clone()))));
                                        }
                                        let asked = frame.get("path").and_then(Value::as_str).unwrap_or("");
                                        let path = if asked.starts_with("/devtools/browser/") { ws.as_str() } else { asked };
                                        cdp(ch, port, path, rx, out, &b)
                                    }
                                    Err(e) => {
                                        let _ = out.send(json!({"op": "close", "ch": ch, "why": format!("the browser did not start again: {e}")}));
                                    }
                                }
                            }
                            Some("pty") => terminal(ch, &frame, rx, out),
                            Some("file") => crate::files::serve(ch, &frame, rx, out),
                            // Signed in with a saved login (logins.rs): answered once, then closed.
                            Some("login") => {
                                let port = lent.as_ref().and_then(|b| b.running()).map(|(p, _)| p);
                                let said = match (port, logins.as_ref()) {
                                    (Some(port), Some(l)) => crate::logins::serve(l, port, &frame),
                                    (None, _) => Err("the browser is not open: open the sign-in page first".into()),
                                    (_, None) => Err("signing agents in is not offered by this client".into()),
                                };
                                let _ = out.send(json!({"op": "opened", "ch": ch}));
                                match said {
                                    Ok(v) => {
                                        let _ = out.send(json!({"op": "msg", "ch": ch, "data": v.to_string()}));
                                        let _ = out.send(json!({"op": "close", "ch": ch, "code": 0}));
                                    }
                                    Err(why) => {
                                        let _ = out.send(json!({"op": "close", "ch": ch, "why": why}));
                                    }
                                }
                            }
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
                    Some("bell") => {
                        let told = told(&frame);
                        let answer: Option<Answer> = if told.ask.is_empty() || told.options.is_empty() {
                            None
                        } else {
                            let (out, ask) = (to_server.clone(), told.ask.clone());
                            Some(Box::new(move |said: String| { let _ = out.send(json!({"op": "answer", "ask": ask, "answer": said})); }))
                        };
                        bell(&told, answer);
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
pub fn keep(setup: &Setup, browser: Option<&Arc<Browser>>, logins: Option<&crate::logins::Logins>, stop: &AtomicBool, say: &dyn Fn(&str), bell: Bell) -> Ended {
    let mut wait = Duration::from_secs(1);
    loop {
        match bridge(setup, browser, logins, stop, say, bell) {
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
fn cdp(ch: u64, port: u16, path: &str, inbox: Receiver<In>, out: Out, lent: &Browser) {
    let _counted = lent.drive();
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
        if lent.stopped() {
            let _ = tab.close(None);
            let _ = tab.flush();
            break "the person stopped agents in this browser".to_string();
        }
        match tab.read() {
            Ok(Message::Text(text)) => {
                lent.touch();
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
    fn agents_in_the_browser_are_counted_and_stopped_by_the_person() {
        let b = Browser::new(PathBuf::from("/nowhere/chrome"), PathBuf::from("/tmp/no-profile"), vec![]);
        assert_eq!(b.agents(), (0, None), "nobody yet");
        {
            let _one = b.drive();
            let _two = b.drive();
            assert_eq!(b.agents().0, 2);
        }
        let (now, since) = b.agents();
        assert_eq!(now, 0, "both gone");
        assert!(since.unwrap() < Duration::from_secs(1), "but one acted just now: still in use for a while");
        assert!(!b.stopped());
        b.stop_agents();
        assert!(b.stopped(), "Stop: every connection ends, none let in again");
    }

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
        let mut setup = Setup { id: String::new(), url: "https://a".into(), key: "ac_k".into(), name: "Mac".into(), offers: Offers::default(), on: false, browser: Default::default() };
        assert_eq!(setup.offers, Offers { browser: false, terminal: false, actions: false });
        setup.offers.terminal = true;
        setup.save(dir.path()).unwrap();
        assert_eq!(Setup::load(dir.path()).unwrap(), setup);
        Setup::forget(dir.path()).unwrap();
        assert!(Setup::load(dir.path()).is_none());
    }

    fn link(url: &str) -> Setup {
        Setup { id: String::new(), url: url.into(), key: "ac_k".into(), name: "Mac".into(), offers: Offers::default(), on: true, browser: Default::default() }
    }

    #[test]
    fn several_argus_are_kept_each_with_its_switches() {
        let dir = tempfile::tempdir().unwrap();
        let mut links = Links::load(dir.path());
        assert!(links.links.is_empty());
        let a = links.add(link("https://vm-one.example:8090"));
        let mut two = link("https://vm-two.example:8090");
        two.offers.terminal = true;
        let b = links.add(two);
        assert_ne!(a, b);
        links.get_mut(&a).unwrap().offers.browser = true;
        links.save(dir.path()).unwrap();
        let back = Links::load(dir.path());
        assert_eq!(back, links);
        assert!(back.get(&a).unwrap().offers.browser && !back.get(&a).unwrap().offers.terminal);
        assert!(back.get(&b).unwrap().offers.terminal && !back.get(&b).unwrap().offers.browser);
        assert!(back.lends_browser());
    }

    #[test]
    fn the_same_argus_linked_again_replaces_its_link() {
        let mut links = Links::default();
        let a = links.add(link("https://vm-one.example:8090"));
        let mut again = link("https://vm-one.example:8090/");
        again.key = "ac_new".into();
        assert_eq!(links.add(again), a);
        assert_eq!(links.links.len(), 1);
        assert_eq!(links.get(&a).unwrap().key, "ac_new");
        assert!(links.remove(&a) && links.links.is_empty());
    }

    #[test]
    fn the_single_link_of_before_is_taken_in() {
        let dir = tempfile::tempdir().unwrap();
        let mut old = link("https://vm-one.example:8090");
        old.browser.flags = "--window-size=1400,900".into();
        old.save(dir.path()).unwrap();
        let links = Links::load(dir.path());
        assert_eq!(links.links.len(), 1);
        assert_eq!(links.links[0].url, "https://vm-one.example:8090");
        assert!(!links.links[0].id.is_empty());
        assert_eq!(links.browser.flags, "--window-size=1400,900", "its browser settings become everybody's");
        assert!(!dir.path().join("computer.json").exists(), "taken in once");
        assert_eq!(Links::load(dir.path()), links);
    }

    #[test]
    fn the_window_key_is_asked_with_the_computers_key_and_never_kept() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            while !String::from_utf8_lossy(&got).contains("\r\n\r\n{}") {
                let n = sock.read(&mut buf).unwrap();
                if n == 0 { break; }
                got.extend_from_slice(&buf[..n]);
            }
            let body = r#"{"key":"0123abcd","device":"d1","name":"Mac (argus-bridge)"}"#;
            write!(sock, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), body).unwrap();
            String::from_utf8_lossy(&got).to_string()
        });
        let dir = tempfile::tempdir().unwrap();
        let mut setup = link(&format!("http://127.0.0.1:{port}/"));
        setup.key = "ac_secret".into();
        assert_eq!(window_key(&setup).unwrap(), "0123abcd");
        let asked = served.join().unwrap().to_lowercase();
        assert!(asked.starts_with("post /api/computers/window-key "), "{asked}");
        assert!(asked.contains("authorization: bearer ac_secret"), "{asked}");
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none(), "nothing written");
    }

    #[test]
    fn a_window_key_refused_is_an_error_not_a_key() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf);
            let _ = write!(sock, "HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
        });
        assert!(window_key(&link(&format!("http://127.0.0.1:{port}"))).is_err());
    }
}
