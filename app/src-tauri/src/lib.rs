//! argus-bridge, the app: the engine of `argus-sync-core` for every connected folder, on a thread of
//! its own, and a window (plain HTML, `../ui`) that shows what it does and asks what to do. It
//! lives in the menu bar / the notification area: closing the window keeps it syncing.

use argus_sync_core::engine::{host_name, Engine, Event, Settings};
use argus_sync_core::names;
use argus_sync_core::pair::{self, Pair, Pairs};
use argus_sync_core::plan::Issue;
use argus_sync_core::remote::{Remote, RemoteError};
use notify::{RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_opener::OpenerExt;

mod words;
use words::{say, tr, tr_n};

const POLL: Duration = Duration::from_secs(5);
const SETTLE: Duration = Duration::from_millis(700);
const RECENT: usize = 60;
const CONFLICT_MARK: &str = " (conflict from ";

// ------------------------------------------------------------------------------- what is shown

#[derive(Debug, Clone, Serialize, Default)]
struct Activity {
    at: String,
    kind: String,
    path: String,
    detail: String,
}

#[derive(Debug, Clone, Serialize, Default)]
struct Waiting {
    path: String,
    size: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
struct Status {
    /// idle | syncing | offline | refused | paused | starting
    state: String,
    message: String,
    last_ok: Option<String>,
    recent: VecDeque<Activity>,
    big: Vec<Waiting>,
    notes: Vec<serde_json::Value>,
    conflicts: Vec<String>,
    write: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
struct PairView {
    id: String,
    name: String,
    url: String,
    host: String,
    local: String,
    paused: bool,
    big_mb: u64,
    status: Status,
}

#[derive(Debug, Clone, Serialize)]
struct Snapshot {
    pairs: Vec<PairView>,
    paused_all: bool,
    computer: String,
    /// This computer, connected to Argus (computer.rs): null when it never was.
    machine: Option<MachineView>,
    /// This copy's version, and a newer one ready to install (null when this is the newest).
    version: String,
    update: Option<String>,
    /// On a Mac: whether argus-bridge has Full Disk Access. Without it macOS stops a command that reads
    /// Documents, Desktop or another app's data to ask "Allow?", and the agent waits for whoever is there.
    full_disk: Option<bool>,
    /// Newer versions installed by themselves (true) or asked first (false).
    auto_update: bool,
    language: String,
}

/// What the window shows of this computer's connection to Argus.
#[derive(Debug, Clone, Serialize)]
struct MachineView {
    host: String,
    name: String,
    offers: argus_sync_core::computer::Offers,
    on: bool,
    /// off | connecting | connected | lost | given-back
    state: String,
    message: String,
    /// How the lent browser starts, and the browsers found here to choose from.
    browser: argus_sync_core::computer::BrowserSettings,
    browsers: Vec<String>,
    /// The logins agents may be signed in with: names and sites only, never a secret.
    logins: LoginsView,
}

#[derive(Debug, Clone, Serialize)]
struct LoginsView {
    enabled: bool,
    onepassword: bool,
    onepassword_installed: bool,
    accounts: Vec<argus_sync_core::logins::Account>,
    always: Vec<String>,
}

fn logins_view(dir: &Path) -> LoginsView {
    let book = argus_sync_core::logins::Vault::new(dir).book();
    LoginsView { enabled: book.enabled, onepassword: book.onepassword,
                 onepassword_installed: argus_sync_core::logins::onepassword::installed(),
                 accounts: book.accounts, always: book.always }
}

/// The connection's own state, kept between its thread and the window.
#[derive(Default)]
struct Machine {
    state: String,
    message: String,
    stop: Option<Arc<std::sync::atomic::AtomicBool>>,
}

#[derive(Debug, Clone, Serialize)]
struct InviteView {
    url: String,
    host: String,
    name: String,
    folder: String,
    write: bool,
    suggested: String,
}

// ------------------------------------------------------------------------------------- shared

enum Wake {
    Now,
}

struct Shared {
    dir: PathBuf,
    status: Mutex<BTreeMap<String, Status>>,
    invite: Mutex<Option<String>>,
    paused_all: Mutex<bool>,
    wake: Mutex<Option<mpsc::Sender<Wake>>>,
    machine: Mutex<Machine>,
    /// A newer version found on GitHub (signed by us — the updater checks), ready to install.
    update: Mutex<Option<String>>,
    updates: Mutex<UpdatePrefs>,
}

/// How newer versions arrive: asked first (the default), or installed by themselves. A version you said
/// "Later" to is not asked again at every hourly look; a press of Check for updates asks it again.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
struct UpdatePrefs {
    #[serde(default)]
    auto: bool,
    #[serde(default)]
    later: Option<String>,
    /// The window's and the menu's language: "" follows the system's.
    #[serde(default)]
    language: String,
}

impl UpdatePrefs {
    fn load(dir: &Path) -> UpdatePrefs {
        std::fs::read(dir.join("updates.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    fn save(&self, dir: &Path) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(dir.join("updates.json"), serde_json::to_vec_pretty(self).unwrap_or_default());
    }
}

impl Shared {
    fn nudge(&self) {
        if let Some(tx) = self.wake.lock().unwrap().as_ref() {
            let _ = tx.send(Wake::Now);
        }
    }
}

fn host_of(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).split(['/', ':']).next().unwrap_or(url).to_string()
}

fn snapshot(shared: &Shared) -> Snapshot {
    // Read once, before the struct below: two `shared.updates.lock()` in one expression hold the first
    // guard while asking for the second, and 0.1.10 hung on itself at its first drawing (2026-11-10).
    let prefs = shared.updates.lock().unwrap().clone();
    let pairs = Pairs::load(&shared.dir).pairs;
    let status = shared.status.lock().unwrap();
    Snapshot {
        pairs: pairs.into_iter().map(|p| PairView {
            status: status.get(&p.id).cloned().unwrap_or_else(|| Status { state: if p.paused { "paused".into() } else { "starting".into() }, ..Default::default() }),
            host: host_of(&p.url),
            id: p.id, name: p.name, url: p.url, local: p.local.display().to_string(), paused: p.paused, big_mb: p.big / (1024 * 1024),
        }).collect(),
        paused_all: *shared.paused_all.lock().unwrap(),
        computer: host_name(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        update: shared.update.lock().unwrap().clone(),
        full_disk: full_disk_access(),
        auto_update: prefs.auto,
        language: prefs.language,
        machine: argus_sync_core::computer::Setup::load(&shared.dir).map(|setup| {
            let m = shared.machine.lock().unwrap();
            MachineView {
                host: host_of(&setup.url), name: setup.name, offers: setup.offers, on: setup.on,
                state: if m.state.is_empty() { if setup.on { "connecting".into() } else { "off".into() } } else { m.state.clone() },
                message: m.message.clone(),
                browsers: argus_sync_core::computer::browsers_here().iter().map(|p| p.display().to_string()).collect(),
                browser: setup.browser,
                logins: logins_view(&shared.dir),
            }
        }),
    }
}

fn publish(app: &AppHandle, shared: &Shared) {
    let snap = snapshot(shared);
    // The tray says the one thing worth knowing at a glance.
    let busy = snap.pairs.iter().filter(|p| p.status.state == "syncing").count();
    let conflicts: usize = snap.pairs.iter().map(|p| p.status.conflicts.len()).sum();
    let off = snap.pairs.iter().filter(|p| matches!(p.status.state.as_str(), "offline" | "refused")).count();
    let tip = if snap.pairs.is_empty() {
        tr("argus-bridge — nothing connected")
    } else if snap.paused_all {
        tr("argus-bridge — paused")
    } else if conflicts > 0 {
        tr_n("argus-bridge — {n} conflict(s) to look at", conflicts)
    } else if off > 0 {
        tr_n("argus-bridge — {n} folder(s) cannot reach the server", off)
    } else if busy > 0 {
        tr("argus-bridge — syncing…")
    } else {
        tr("argus-bridge — up to date")
    };
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(&tip));
    }
    let _ = app.emit("snapshot", snap);
}

/// What a failure says, for the window to put in the person's language.
fn remote_say(e: &RemoteError) -> String {
    match e {
        RemoteError::Network(why) => say("Cannot reach the server: {why}", &[("why", why.clone())]),
        RemoteError::Unauthorized => say("The server no longer accepts this key — connect the folder again from Argus", &[]),
        RemoteError::Refused(code, why) => say("The server refused ({code}): {why}", &[("code", code.to_string()), ("why", why.clone())]),
        RemoteError::Conflict(why) => say("Changed on the server meanwhile: {why}", &[("why", why.clone())]),
        RemoteError::Io(why) => say("Something went wrong: {why}", &[("why", why.to_string())]),
    }
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn activity(ev: &Event) -> Activity {
    let (kind, path, detail) = match ev {
        Event::Downloaded { path, bytes } => ("down", path.clone(), bytes.to_string()),
        Event::Uploaded { path, bytes } => ("up", path.clone(), bytes.to_string()),
        Event::DeletedHere { path } => ("deleted-here", path.clone(), String::new()),
        Event::DeletedThere { path } => ("deleted-there", path.clone(), String::new()),
        Event::FolderHere { path } => ("folder-here", path.clone(), String::new()),
        Event::FolderThere { path } => ("folder-there", path.clone(), String::new()),
        Event::Conflict { path, copy } => ("conflict", path.clone(), copy.clone()),
        Event::Later { path, why } => ("later", path.clone(), why.clone()),
    };
    Activity { at: now(), kind: kind.into(), path, detail }
}

fn note(issue: &Issue) -> Option<serde_json::Value> {
    // Structured, so the window says it in the person's language.
    Some(match issue {
        Issue::Big { .. } => return None,
        Issue::BadName { remote, why } => serde_json::json!({"kind": "name", "path": remote, "why": why}),
        Issue::CaseClash { kept, skipped } => serde_json::json!({"kind": "case", "kept": kept, "skipped": skipped}),
        Issue::FileVersusFolder { path } => serde_json::json!({"kind": "type", "path": path}),
        Issue::HeldDeletes { count } => serde_json::json!({"kind": "held", "count": count}),
    })
}

/// The conflict copies in the folder: what argus-bridge made, still waiting for a decision.
fn conflicts_in(engine: &Engine) -> Vec<String> {
    let mut out: Vec<String> = engine.state.files.values()
        .filter(|k| !k.dir && k.local.contains(CONFLICT_MARK))
        .map(|k| k.local.clone())
        .filter(|rel| argus_sync_core::local::join(&engine.settings.root, rel).exists())
        .collect();
    out.sort();
    out
}

// ------------------------------------------------------------------------------------- worker

fn worker(app: AppHandle, shared: Arc<Shared>, rx: mpsc::Receiver<Wake>) {
    let (ftx, frx) = mpsc::channel::<Vec<PathBuf>>();
    let mut watcher = notify::recommended_watcher(move |ev: notify::Result<notify::Event>| {
        if let Ok(ev) = ev {
            let _ = ftx.send(ev.paths);
        }
    }).ok();
    let mut watched: BTreeSet<PathBuf> = BTreeSet::new();
    let mut engines: HashMap<String, (Engine, Instant, bool)> = HashMap::new();
    let mut local_only_conflicts: HashMap<String, BTreeSet<String>> = HashMap::new();

    loop {
        let pairs = Pairs::load(&shared.dir).pairs;
        let paused_all = *shared.paused_all.lock().unwrap();
        engines.retain(|id, _| pairs.iter().any(|p| &p.id == id));
        shared.status.lock().unwrap().retain(|id, _| pairs.iter().any(|p| &p.id == id));
        // Watch what is connected, and only that.
        let want: BTreeSet<PathBuf> = pairs.iter().map(|p| p.local.clone()).collect();
        if let Some(w) = watcher.as_mut() {
            for gone in watched.difference(&want).cloned().collect::<Vec<_>>() {
                let _ = w.unwatch(&gone);
                watched.remove(&gone);
            }
            for new in want.difference(&watched).cloned().collect::<Vec<_>>() {
                let _ = std::fs::create_dir_all(&new);
                if w.watch(&new, RecursiveMode::Recursive).is_ok() {
                    watched.insert(new);
                }
            }
        }

        for p in &pairs {
            let entry = engines.entry(p.id.clone()).or_insert_with(|| (make_engine(&shared.dir, p), Instant::now() - POLL, true));
            // Moved on this computer: the engine follows. Kept on the old place, it found that folder gone —
            // and, before 0.1.7, made it again empty and deleted every file on the server.
            if entry.0.settings.root != p.local {
                *entry = (make_engine(&shared.dir, p), Instant::now() - POLL, true);
            }
            entry.0.settings.big = p.big;
            entry.0.settings.wanted = p.wanted.clone();
            if p.paused || paused_all {
                set(&shared, &p.id, |s| { s.state = "paused".into(); s.message.clear(); });
                continue;
            }
            let (engine, last, dirty) = entry;
            if !*dirty && last.elapsed() < POLL {
                continue;
            }
            *last = Instant::now();
            *dirty = false;
            set(&shared, &p.id, |s| if s.state != "syncing" { s.state = "syncing".into() });
            let mut seen: Vec<Activity> = Vec::new();
            let mut copies: Vec<String> = Vec::new();
            let result = engine.sync_once(&mut |ev| {
                if let Event::Conflict { copy, .. } = ev {
                    copies.push(copy.clone());
                }
                seen.push(activity(ev));
            });
            let mine = local_only_conflicts.entry(p.id.clone()).or_default();
            mine.extend(copies);
            mine.retain(|rel| argus_sync_core::local::join(&p.local, rel).exists());
            let mut conflicts = conflicts_in(engine);
            conflicts.extend(mine.iter().cloned());
            conflicts.sort();
            conflicts.dedup();
            let was_idle = seen.is_empty();
            set(&shared, &p.id, |s| {
                for a in seen {
                    s.recent.push_front(a);
                }
                s.recent.truncate(RECENT);
                s.conflicts = conflicts;
                match &result {
                    Ok(r) => {
                        s.state = "idle".into();
                        s.message.clear();
                        s.last_ok = Some(now());
                        s.big = r.issues.iter().filter_map(|i| match i { Issue::Big { remote, size } => Some(Waiting { path: remote.clone(), size: *size }), _ => None }).collect();
                        s.notes = r.issues.iter().filter_map(note).collect();
                    }
                    Err(e) => {
                        s.state = if matches!(e, RemoteError::Unauthorized) { "refused" } else { "offline" }.into();
                        s.message = remote_say(e);
                    }
                }
            });
            if let Ok(h) = engine.remote.hello() {
                set(&shared, &p.id, |s| s.write = Some(h.write));
            }
            publish(&app, &shared);
            // Something moved: look again soon, there is often a second half to it.
            if !was_idle {
                *dirty = true;
            }
        }
        publish(&app, &shared);

        // Wait for a change here, a press in the window, or the next look at the servers.
        let deadline = Instant::now() + Duration::from_millis(500);
        if rx.recv_timeout(Duration::from_millis(10)).is_ok() {
            for (e, _, d) in engines.values_mut() {
                let _ = e;
                *d = true;
            }
            continue;
        }
        if let Ok(paths) = frx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            let mut touched = paths;
            let until = Instant::now() + SETTLE;
            while let Ok(more) = frx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                touched.extend(more);
            }
            for p in &pairs {
                if touched.iter().any(|t| t.starts_with(&p.local) && !t.file_name().map(|n| names::ignored(&n.to_string_lossy())).unwrap_or(false)) {
                    if let Some(e) = engines.get_mut(&p.id) {
                        e.2 = true;
                    }
                }
            }
        }
    }
}

fn set(shared: &Shared, id: &str, f: impl FnOnce(&mut Status)) {
    let mut all = shared.status.lock().unwrap();
    f(all.entry(id.to_string()).or_default());
}

fn make_engine(dir: &Path, p: &Pair) -> Engine {
    Engine::new(Remote::new(&p.url, &p.key), Settings {
        root: p.local.clone(),
        state_file: pair::state_file(dir, p),
        fallback_trash: pair::fallback_trash(dir, p),
        big: p.big,
        wanted: p.wanted.clone(),
        host: host_name(),
    })
}

// ----------------------------------------------------------------------------------- commands

type Res<T> = Result<T, String>;

fn edit(shared: &Shared, id: &str, f: impl FnOnce(&mut Pair) -> Res<()>) -> Res<()> {
    let mut pairs = Pairs::load(&shared.dir);
    let p = pairs.pairs.iter_mut().find(|p| p.id == id).ok_or_else(|| say("That folder is not connected any more", &[]))?;
    f(p)?;
    pairs.save(&shared.dir).map_err(|e| e.to_string())?;
    shared.nudge();
    Ok(())
}

fn pair_of(shared: &Shared, id: &str) -> Res<Pair> {
    Pairs::load(&shared.dir).pairs.into_iter().find(|p| p.id == id).ok_or_else(|| say("That folder is not connected any more", &[]))
}

#[tauri::command]
fn get_snapshot(shared: tauri::State<Arc<Shared>>) -> Snapshot {
    snapshot(&shared)
}

#[tauri::command]
fn take_invite(shared: tauri::State<Arc<Shared>>) -> Option<String> {
    shared.invite.lock().unwrap().take()
}

/// What a link opens, asked of the server before anything is written: so the window can say
/// "proj on lab-server, both ways" and suggest where to put it.
#[tauri::command]
async fn inspect(link: String) -> Res<InviteView> {
    let invite = pair::parse_link(&link).ok_or_else(|| say("That is not a link from Argus — it starts with argus-bridge://", &[]))?;
    let url = invite.url.clone();
    let key = invite.key.clone();
    let hello = tauri::async_runtime::spawn_blocking(move || Remote::new(&url, &key).hello())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| remote_say(&e))?;
    let base = dirs::home_dir().unwrap_or_default().join("argus-sync");
    let mut suggested = base.join(&hello.folder);
    let mut n = 2;
    while suggested.exists() && std::fs::read_dir(&suggested).map(|mut d| d.next().is_some()).unwrap_or(false) {
        suggested = base.join(format!("{} {n}", hello.folder));
        n += 1;
    }
    Ok(InviteView {
        url: invite.url, host: hello.host, name: if invite.name.is_empty() { hello.folder.clone() } else { invite.name },
        folder: hello.folder, write: hello.write, suggested: suggested.display().to_string(),
    })
}

#[tauri::command]
fn connect(shared: tauri::State<Arc<Shared>>, link: String, local: String) -> Res<String> {
    let invite = pair::parse_link(&link).ok_or_else(|| say("That is not a link from Argus — it starts with argus-bridge://", &[]))?;
    let local = PathBuf::from(local);
    if !local.is_absolute() {
        return Err(say("Choose a folder on this computer", &[]));
    }
    let mut pairs = Pairs::load(&shared.dir);
    if let Some(p) = pairs.pairs.iter().find(|p| p.local == local || local.starts_with(&p.local) || p.local.starts_with(&local)) {
        return Err(say("{path} is already in sync with {name} — choose another folder", &[("path", local.display().to_string()), ("name", p.name.clone())]));
    }
    std::fs::create_dir_all(&local).map_err(|e| say("Cannot make {path}: {why}", &[("path", local.display().to_string()), ("why", e.to_string())]))?;
    let id = pairs.add(&invite, local).id.clone();
    pairs.save(&shared.dir).map_err(|e| e.to_string())?;
    shared.nudge();
    Ok(id)
}

#[tauri::command]
fn set_paused(shared: tauri::State<Arc<Shared>>, id: String, paused: bool) -> Res<()> {
    edit(&shared, &id, |p| { p.paused = paused; Ok(()) })
}

#[tauri::command]
fn pause_all(app: AppHandle, shared: tauri::State<Arc<Shared>>, paused: bool) {
    *shared.paused_all.lock().unwrap() = paused;
    shared.nudge();
    publish(&app, &shared);
}

#[tauri::command]
fn set_big(shared: tauri::State<Arc<Shared>>, id: String, mb: u64) -> Res<()> {
    edit(&shared, &id, |p| { p.big = mb * 1024 * 1024; Ok(()) })
}

#[tauri::command]
fn fetch(shared: tauri::State<Arc<Shared>>, id: String, path: String) -> Res<()> {
    edit(&shared, &id, |p| { p.wanted.insert(names::key(&path)); Ok(()) })
}

#[tauri::command]
fn sync_now(shared: tauri::State<Arc<Shared>>) {
    shared.nudge();
}

/// Stop syncing a folder. The files stay where they are, on both sides.
#[tauri::command]
fn disconnect(shared: tauri::State<Arc<Shared>>, id: String) -> Res<()> {
    let mut pairs = Pairs::load(&shared.dir);
    let gone = pairs.pairs.iter().find(|p| p.id == id).cloned().ok_or_else(|| say("That folder is not connected any more", &[]))?;
    pairs.pairs.retain(|p| p.id != id);
    pairs.save(&shared.dir).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(pair::state_file(&shared.dir, &gone));
    shared.nudge();
    Ok(())
}

/// Move the folder on this computer — by renaming it, so nothing is downloaded again.
#[tauri::command]
fn move_local(shared: tauri::State<Arc<Shared>>, id: String, to: String) -> Res<()> {
    let p = pair_of(&shared, &id)?;
    let to = PathBuf::from(to);
    let dest = if to.exists() && to.is_dir() && std::fs::read_dir(&to).map(|mut d| d.next().is_some()).unwrap_or(false) {
        to.join(p.local.file_name().unwrap_or_default())
    } else {
        to
    };
    if dest.exists() && std::fs::read_dir(&dest).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return Err(say("{path} is not empty", &[("path", dest.display().to_string())]));
    }
    let _ = std::fs::remove_dir(&dest);
    std::fs::rename(&p.local, &dest).map_err(|e| say("Could not move it there ({why}) — a folder on the same disk works", &[("why", e.to_string())]))?;
    edit(&shared, &id, |x| { x.local = dest; Ok(()) })
}

/// A conflict copy, decided: keep mine (it replaces the other), keep the server's (mine goes to
/// the trash), or keep both (the copy loses its "conflict" in the name).
#[tauri::command]
fn resolve(shared: tauri::State<Arc<Shared>>, id: String, copy: String, choice: String) -> Res<()> {
    let p = pair_of(&shared, &id)?;
    let copy_path = argus_sync_core::local::join(&p.local, &copy);
    let start = copy.find(CONFLICT_MARK).ok_or_else(|| say("That is not a conflict copy", &[]))?;
    let end = copy[start..].find(')').map(|i| start + i + 1).ok_or_else(|| say("That is not a conflict copy", &[]))?;
    let original = format!("{}{}", &copy[..start], &copy[end..]);
    let original_path = argus_sync_core::local::join(&p.local, &original);
    match choice.as_str() {
        "mine" => std::fs::rename(&copy_path, &original_path).map_err(|e| e.to_string())?,
        "theirs" => trash::delete(&copy_path).or_else(|_| std::fs::remove_file(&copy_path)).map_err(|e| e.to_string())?,
        "both" => {
            let host: String = host_name();
            let kept = format!("{} (from {host}){}", &copy[..start], &copy[end..]);
            std::fs::rename(&copy_path, argus_sync_core::local::join(&p.local, &kept)).map_err(|e| e.to_string())?
        }
        _ => return Err(say("That is not a conflict copy", &[])),
    }
    shared.nudge();
    Ok(())
}

/// Open a synced folder, or a file in it, with the system (Finder, Explorer, the file manager).
/// Only inside a connected folder: the window cannot ask for anything else.
#[tauri::command]
fn open_in(app: AppHandle, shared: tauri::State<Arc<Shared>>, id: String, rel: Option<String>) -> Res<()> {
    let p = pair_of(&shared, &id)?;
    let rel = rel.unwrap_or_default();
    if rel.split('/').any(|part| part == "..") {
        return Err(say("That is not inside the synced folder", &[]));
    }
    let path = if rel.is_empty() { p.local.clone() } else { argus_sync_core::local::join(&p.local, &rel) };
    if !path.exists() {
        return Err(say("{path} is not there", &[("path", path.display().to_string())]));
    }
    app.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(|e| say("Something went wrong: {why}", &[("why", e.to_string())]))
}

// ------------------------------------------------------------------------------- this computer

/// (Re)start this computer's connection to Argus as its setup says: stopped first, then — when it
/// is on — a Chrome of its own if the browser is lent, and the bridge, made again when it drops.
fn start_machine(app: &AppHandle, shared: &Arc<Shared>) {
    use argus_sync_core::computer::{self, Ended, Setup};
    use std::sync::atomic::{AtomicBool, Ordering};
    if let Some(stop) = shared.machine.lock().unwrap().stop.take() {
        stop.store(true, Ordering::Relaxed);
    }
    let Some(setup) = Setup::load(&shared.dir) else {
        *shared.machine.lock().unwrap() = Machine::default();
        publish(app, shared);
        return;
    };
    if !setup.on {
        *shared.machine.lock().unwrap() = Machine { state: "off".into(), ..Default::default() };
        publish(app, shared);
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    *shared.machine.lock().unwrap() = Machine { state: "connecting".into(), message: String::new(), stop: Some(stop.clone()) };
    publish(app, shared);
    let (app, shared) = (app.clone(), shared.clone());
    std::thread::spawn(move || {
        let set = |state: &str, message: String| {
            let mut m = shared.machine.lock().unwrap();
            if m.stop.as_ref().map(|s| Arc::ptr_eq(s, &stop)).unwrap_or(false) {
                m.state = state.into();
                m.message = message;
            }
            drop(m);
            publish(&app, &shared);
        };
        let browser = if setup.offers.browser {
            match setup.browser.exe() {
                None => {
                    set("connecting", say("No Chrome here: the browser is not lent (install Chrome, or switch Browser off)", &[]));
                    None
                }
                // Started by the bridge, and again whenever an agent needs it after the window was closed.
                Some(exe) => Some(Arc::new(computer::Browser::new(exe, shared.dir.join("browser-profile"), setup.browser.args()))),
            }
        } else {
            None
        };
        let said = |line: &str| {
            if line.starts_with("connected to") {
                set("connected", String::new());
            } else if line.starts_with("connection to") {
                set("lost", line.to_string());
            }
        };
        // A bell of Argus — an agent finished, failed or asks — as a notification of this system.
        // A question (Do it / No) is answered in the notification itself, where the system can.
        let bell_app = app.clone();
        let bell_shared = shared.clone();
        let bell = move |told: &computer::Told, answer: Option<computer::Answer>| {
            use tauri_plugin_notification::NotificationExt;
            let title = match told.why.as_str() {
                "asking" => say_plain("{session} is waiting for you", &told.session),
                "failed" => say_plain("{session} failed", &told.session),
                _ => say_plain("{session} has finished", &told.session),
            };
            if let Some(answer) = answer {
                if ask_in_notification(&bell_app, &bell_shared, title.clone(), told, answer) {
                    return;
                }
            }
            let _ = bell_app.notification().builder().title(title).body(&told.text).show();
        };
        // An agent asking to be signed in: asked here, on this computer, in a dialog of the system — the
        // site, the account and where it is kept; Allow once, Always for this site, or No.
        let ask_app = app.clone();
        let ask: argus_sync_core::logins::AskLogin = Arc::new(move |site: &str, account: &argus_sync_core::logins::Account| {
            use argus_sync_core::logins::Decision;
            use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogResult};
            let from = if account.source == "1password" { "1Password" } else { "argus-bridge" };
            let (once, always, no) = (words::tr("Allow once"), words::tr("Always for this site"), words::tr("No"));
            let said = ask_app.dialog()
                .message(words::tr("An agent asks to be signed in to {site} as {account}, with the login kept in {from}. It never sees the password.")
                    .replace("{site}", site).replace("{account}", &account.username).replace("{from}", from))
                .title(words::tr("Sign an agent in?"))
                .buttons(MessageDialogButtons::YesNoCancelCustom(once.clone(), always.clone(), no.clone()))
                .blocking_show_with_result();
            match said {
                MessageDialogResult::Custom(l) if l == once => Decision::Once,
                MessageDialogResult::Custom(l) if l == always => Decision::Always,
                MessageDialogResult::Yes => Decision::Once,
                _ => Decision::No,
            }
        });
        let logins = argus_sync_core::logins::Logins { dir: shared.dir.clone(), ask: Some(ask) };
        let ended = computer::keep(&setup, browser.as_ref(), Some(&logins), &stop, &said, &bell);
        if let Some(b) = &browser {
            b.stop();
        }
        match ended {
            Ended::GivenBack => {
                // Disconnected in Argus: off here too, until switched on again.
                let mut off = setup.clone();
                off.on = false;
                let _ = off.save(&shared.dir);
                set("given-back", String::new());
            }
            Ended::Stopped => {}
            Ended::Lost(why) => set("lost", why),
        }
    });
}

/// A link from Argus to connect this computer: kept, with every switch off until you turn it on.
#[tauri::command]
fn machine_link(app: AppHandle, shared: tauri::State<Arc<Shared>>, link: String, browser: bool, terminal: bool, actions: bool) -> Res<()> {
    use argus_sync_core::computer::{self, Offers, Setup};
    let invite = computer::parse_link(&link).ok_or_else(|| say("That is not a link from Argus to connect a computer", &[]))?;
    let setup = Setup {
        url: invite.url, key: invite.key, name: if invite.name.is_empty() { host_name() } else { invite.name },
        offers: Offers { browser, terminal, actions }, on: true,
        // Linked again: how its browser starts stays as the person set it.
        browser: Setup::load(&shared.dir).map(|s| s.browser).unwrap_or_default(),
    };
    setup.save(&shared.dir).map_err(|e| e.to_string())?;
    start_machine(&app, shared.inner());
    Ok(())
}

/// The switches, and whether it is connected at all.
#[tauri::command]
fn machine_set(app: AppHandle, shared: tauri::State<Arc<Shared>>, browser: bool, terminal: bool, actions: bool, on: bool) -> Res<()> {
    use argus_sync_core::computer::{Offers, Setup};
    let mut setup = Setup::load(&shared.dir).ok_or_else(|| say("This computer is not connected to Argus", &[]))?;
    setup.offers = Offers { browser, terminal, actions };
    setup.on = on;
    setup.save(&shared.dir).map_err(|e| e.to_string())?;
    start_machine(&app, shared.inner());
    Ok(())
}

/// How the lent browser starts — chosen by the person, used from its next start (the connection is made
/// again, and with it the browser, which an agent's next call starts with these).
#[tauri::command]
fn machine_browser(app: AppHandle, shared: tauri::State<Arc<Shared>>, hide_automation: bool, exe: String, flags: String) -> Res<()> {
    use argus_sync_core::computer::{BrowserSettings, Setup};
    let mut setup = Setup::load(&shared.dir).ok_or_else(|| say("This computer is not connected to Argus", &[]))?;
    if !exe.trim().is_empty() && !std::path::Path::new(exe.trim()).is_file() {
        return Err(say("There is no browser at {path}", &[("path", exe.trim().to_string())]));
    }
    setup.browser = BrowserSettings { hide_automation, exe: exe.trim().to_string(), flags: flags.trim().to_string() };
    setup.save(&shared.dir).map_err(|e| e.to_string())?;
    start_machine(&app, shared.inner());
    Ok(())
}

// ------------------------------------------------------------------------------------- logins

/// Signing agents in at all, and with 1Password too.
#[tauri::command]
fn logins_set(app: AppHandle, shared: tauri::State<Arc<Shared>>, enabled: bool, onepassword: bool) -> Res<()> {
    let vault = argus_sync_core::logins::Vault::new(&shared.dir);
    let mut book = vault.book();
    book.enabled = enabled;
    book.onepassword = onepassword;
    vault.save_book(&book).map_err(|e| e.to_string())?;
    publish(&app, &shared);
    Ok(())
}

/// A login kept by hand: its secret goes straight into the system's keychain.
#[tauri::command]
fn login_add(app: AppHandle, shared: tauri::State<Arc<Shared>>, site: String, username: String, password: String, otp: String) -> Res<()> {
    argus_sync_core::logins::Vault::new(&shared.dir).add(&site, &username, &password, Some(otp.as_str()))
        .map_err(|e| e)?;
    publish(&app, &shared);
    Ok(())
}

#[tauri::command]
fn login_remove(app: AppHandle, shared: tauri::State<Arc<Shared>>, site: String, username: String) -> Res<()> {
    argus_sync_core::logins::Vault::new(&shared.dir).remove(&site, &username).map_err(|e| e.to_string())?;
    publish(&app, &shared);
    Ok(())
}

/// A site no longer signed in without asking.
#[tauri::command]
fn login_forget_always(app: AppHandle, shared: tauri::State<Arc<Shared>>, site: String) -> Res<()> {
    let vault = argus_sync_core::logins::Vault::new(&shared.dir);
    let mut book = vault.book();
    book.always.retain(|s| s != &site);
    vault.save_book(&book).map_err(|e| e.to_string())?;
    publish(&app, &shared);
    Ok(())
}

/// A password manager's export (CSV) taken into the keychain — and the file, which holds every password
/// in the clear, moved to the trash after: offered, not done behind the person's back.
#[tauri::command]
async fn logins_import(app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) -> Res<Option<usize>> {
    use tauri_plugin_dialog::DialogExt;
    let Some(picked) = app.dialog().file().add_filter("CSV", &["csv"]).blocking_pick_file() else { return Ok(None) };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    let kept = argus_sync_core::logins::Vault::new(&shared.dir).import_csv(&path).map_err(|e| e)?;
    publish(&app, &shared);
    use tauri_plugin_dialog::MessageDialogButtons;
    let bin = app.dialog().message(words::tr("{n} logins are now in the keychain. The exported file still holds every password in the clear: move it to the trash?").replace("{n}", &kept.to_string()))
        .title("argus-bridge").buttons(MessageDialogButtons::OkCancelCustom(words::tr("Move it to the trash"), words::tr("Keep it")))
        .blocking_show();
    if bin {
        let _ = trash::delete(&path);
    }
    Ok(Some(kept))
}

/// Forget the connection here (Argus keeps the computer listed until it is forgotten there too).
#[tauri::command]
fn machine_forget(app: AppHandle, shared: tauri::State<Arc<Shared>>) -> Res<()> {
    use argus_sync_core::computer::Setup;
    if let Some(stop) = shared.machine.lock().unwrap().stop.take() {
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    Setup::forget(&shared.dir).map_err(|e| e.to_string())?;
    start_machine(&app, shared.inner());
    Ok(())
}

/// A notification's title in the system's language (the window is not there to translate it).
fn say_plain(template: &str, session: &str) -> String {
    let who = if session.is_empty() { "Argus" } else { session };
    words::tr(template).replace("{session}", who)
}

/// Whether this app may read what macOS protects: Safari's folder is there on every Mac and opens only
/// with Full Disk Access. None elsewhere.
fn full_disk_access() -> Option<bool> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let safari = dirs::home_dir()?.join("Library/Safari");
    Some(std::fs::read_dir(safari).is_ok())
}

/// The Privacy pane where Full Disk Access is given (System Settings opens on it).
#[tauri::command]
fn open_full_disk_settings(app: AppHandle) -> Res<()> {
    app.opener().open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles", None::<&str>)
        .map_err(|e| say("Something went wrong: {why}", &[("why", e.to_string())]))
}

/// A question of Argus as a notification with its answers on it: the first option ("Do it") is the
/// notification's button, the second ("No") its close button, and a click on the text opens Argus.
/// Only on a Mac (NSUserNotification); elsewhere false, and the question is shown as a plain
/// notification, answered in Argus.
#[cfg(target_os = "macos")]
fn ask_in_notification(app: &AppHandle, shared: &Arc<Shared>, title: String, told: &argus_sync_core::computer::Told, answer: argus_sync_core::computer::Answer) -> bool {
    use mac_notification_sys::{MainButton, Notification, NotificationResponse};
    let (Some(yes), Some(no)) = (told.options.first().cloned(), told.options.get(1).cloned()) else { return false };
    let (text, app, shared) = (told.text.clone(), app.clone(), shared.clone());
    std::thread::spawn(move || {
        let _ = mac_notification_sys::set_application(&app.config().identifier);
        let said = Notification::new().title(&title).message(&text)
            .main_button(MainButton::SingleAction(&yes)).close_button(&no).wait_for_click(true).send();
        match said {
            Ok(NotificationResponse::ActionButton(_)) => answer(yes),
            Ok(NotificationResponse::CloseButton(_)) => answer(no),
            Ok(NotificationResponse::Click) => {
                let _ = open_argus_window(&app, &shared);
            }
            _ => {}
        }
    });
    true
}

#[cfg(not(target_os = "macos"))]
fn ask_in_notification(_: &AppHandle, _: &Arc<Shared>, _: String, _: &argus_sync_core::computer::Told, _: argus_sync_core::computer::Answer) -> bool {
    false
}

/// Argus itself, in a window of this app: a Dock icon of its own, no browser tabs. The address is
/// the server this computer (or else the first folder) is connected to; you sign in once there.
///
/// Async on purpose: a window made inside a synchronous command waits on the thread that has to draw it,
/// and "Open Argus" sometimes took seconds, or a second press, to appear (2026-11-09).
#[tauri::command]
async fn open_argus(app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) -> Res<()> {
    open_argus_window(&app, &shared)
}

/// Bring this app to the front: pressed from the menu-bar icon, macOS shows a window of an app that is
/// not the active one behind the others — the window was there, just not in front.
fn to_front(app: &AppHandle, w: &tauri::WebviewWindow) {
    #[cfg(target_os = "macos")]
    let _ = app.show();
    #[cfg(not(target_os = "macos"))]
    let _ = app;
    let _ = w.show();
    let _ = w.unminimize();
    let _ = w.set_focus();
}

fn open_argus_window(app: &AppHandle, shared: &Shared) -> Res<()> {
    if let Some(w) = app.get_webview_window("argus") {
        to_front(app, &w);
        return Ok(());
    }
    let url = argus_sync_core::computer::Setup::load(&shared.dir).map(|s| s.url)
        .or_else(|| Pairs::load(&shared.dir).pairs.first().map(|p| p.url.clone()))
        .ok_or_else(|| say("Connect this computer or a folder first: then Argus opens here", &[]))?;
    let parsed = url.parse::<tauri::Url>().map_err(|e| e.to_string())?;
    tauri::WebviewWindowBuilder::new(app, "argus", tauri::WebviewUrl::External(parsed))
        .title("Argus")
        .inner_size(1360.0, 880.0)
        .min_inner_size(420.0, 420.0)
        .focused(true)
        // A click on a window that is not in front is a click, not only "bring it forward" (macOS).
        .accept_first_mouse(true)
        // Argus's own ground while it loads, not a white rectangle that reads as nothing opening.
        .background_color(tauri::window::Color(11, 14, 20, 255))
        .build()
        .map_err(|e| e.to_string())
        .map(|w| to_front(app, &w))
}

// ------------------------------------------------------------------------------- updating itself

const UPDATE_EVERY: Duration = Duration::from_secs(3600);

/// Ask GitHub whether a newer version is out. The updater only accepts one signed by the key whose
/// public half is in tauri.conf.json: a release nobody signed is never offered, let alone installed.
///
/// Asked first unless you chose otherwise ("someone may not want it automatic", 2026-11-09): a dialog of
/// the system, Update and restart or Later. `asked` is a press of Check for updates — it asks even a
/// version you said Later to, and says so when there is nothing newer.
async fn look_for_update(app: &AppHandle, shared: &Arc<Shared>, asked: bool) -> Res<Option<String>> {
    use tauri_plugin_updater::UpdaterExt;
    // In words, not the updater's: "could not fetch a valid release JSON" was all a press of Check for
    // updates said while a release was being published (2026-11-10).
    let update = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| {
        say("Could not check for updates: GitHub did not answer as expected ({why}). Try again in a few minutes.", &[("why", e.to_string())])
    })?;
    let found = update.as_ref().map(|u| u.version.clone());
    let changed = *shared.update.lock().unwrap() != found;
    *shared.update.lock().unwrap() = found.clone();
    if changed {
        publish(app, shared);
    }
    let prefs = shared.updates.lock().unwrap().clone();
    match (&found, prefs.auto && !asked) {
        (Some(_), true) => install(app, update.unwrap()).await,
        (Some(v), false) if asked || prefs.later.as_deref() != Some(v.as_str()) => offer(app, shared, v.clone()),
        (None, _) if asked => {
            use tauri_plugin_dialog::DialogExt;
            app.dialog().message(words::tr("You have the newest version ({version}).").replace("{version}", env!("CARGO_PKG_VERSION")))
                .title("argus-bridge").show(|_| {});
        }
        _ => {}
    }
    Ok(found)
}

/// "argus-bridge 0.1.10 is out (you have 0.1.9). Update now?" — Update and restart, or Later.
fn offer(app: &AppHandle, shared: &Arc<Shared>, version: String) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
    let text = words::tr("argus-bridge {version} is out (you have {current}). Update now? It restarts by itself; your folders and this computer's connection carry on.")
        .replace("{version}", &version).replace("{current}", env!("CARGO_PKG_VERSION"));
    let (app2, shared2) = (app.clone(), shared.clone());
    app.dialog().message(text).title(words::tr("A new version of argus-bridge"))
        .buttons(MessageDialogButtons::OkCancelCustom(words::tr("Update and restart"), words::tr("Later")))
        .show(move |yes| {
            if yes {
                tauri::async_runtime::spawn(async move {
                    use tauri_plugin_updater::UpdaterExt;
                    if let Ok(Some(u)) = match app2.updater() { Ok(up) => up.check().await, Err(e) => Err(e) } {
                        install(&app2, u).await;
                    }
                });
            } else {
                let mut prefs = shared2.updates.lock().unwrap();
                prefs.later = Some(version);
                prefs.save(&shared2.dir);
            }
        });
}

async fn install(app: &AppHandle, update: tauri_plugin_updater::Update) {
    use tauri_plugin_notification::NotificationExt;
    let version = update.version.clone();
    if update.download_and_install(|_, _| {}, || {}).await.is_ok() {
        let _ = app.notification().builder().title("argus-bridge")
            .body(words::tr("Updated to {version}").replace("{version}", &version)).show();
        std::thread::sleep(Duration::from_secs(2));
        app.restart();
    }
}

/// Install the newer version and start again with it: one press in the window, or the tray.
#[tauri::command]
async fn update_now(app: AppHandle) -> Res<()> {
    use tauri_plugin_updater::UpdaterExt;
    let update = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?
        .ok_or_else(|| say("This is the newest version", &[]))?;
    update.download_and_install(|_, _| {}, || {}).await.map_err(|e| say("The update did not install: {why}", &[("why", e.to_string())]))?;
    app.restart();
}

/// Check for updates, pressed: asks GitHub now, and answers either way.
#[tauri::command]
async fn check_updates(app: AppHandle, shared: tauri::State<'_, Arc<Shared>>) -> Res<Option<String>> {
    look_for_update(&app, shared.inner(), true).await
}

/// The language of the window and of the menu ("" = the system's). The window reloads itself; the
/// menu is made again here.
#[tauri::command]
fn set_language(app: AppHandle, shared: tauri::State<Arc<Shared>>, lang: String) -> Res<()> {
    let lang = if ["en", "it", "es", "fr"].contains(&lang.as_str()) { lang } else { String::new() };
    let mut prefs = shared.updates.lock().unwrap();
    prefs.language = lang.clone();
    prefs.save(&shared.dir);
    drop(prefs);
    words::choose(&lang);
    if let (Some(tray), Ok(menu)) = (app.tray_by_id("main"), tray_menu(&app)) {
        let _ = tray.set_menu(Some(menu));
    }
    publish(&app, &shared);
    Ok(())
}

fn tray_menu<R: tauri::Runtime, M: tauri::Manager<R>>(app: &M) -> tauri::Result<Menu<R>> {
    let argus = MenuItem::with_id(app, "argus", tr("Open Argus"), true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", tr("Open argus-bridge"), true, None::<&str>)?;
    let now = MenuItem::with_id(app, "now", tr("Sync now"), true, None::<&str>)?;
    let check = MenuItem::with_id(app, "check", tr("Check for updates…"), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", tr("Quit"), true, None::<&str>)?;
    Menu::with_items(app, &[&argus, &open, &now, &check, &quit])
}

/// Installed by themselves, or asked first.
#[tauri::command]
fn set_auto_update(app: AppHandle, shared: tauri::State<Arc<Shared>>, on: bool) {
    let mut prefs = shared.updates.lock().unwrap();
    prefs.auto = on;
    prefs.save(&shared.dir);
    drop(prefs);
    publish(&app, &shared);
}

// ----------------------------------------------------------------------------------------- app

fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        to_front(app, &w);
    }
}

fn take_links(app: &AppHandle, shared: &Shared, urls: Vec<String>) {
    for url in urls.into_iter().filter(|u| pair::is_our_link(u)) {
        *shared.invite.lock().unwrap() = Some(url.clone());
        // A computer's link and a folder's look alike to the system; the window tells them apart.
        let _ = app.emit("invite", url);
    }
    show(app);
}

pub fn run() {
    let dir = pair::config_dir();
    let shared = Arc::new(Shared {
        dir,
        status: Mutex::new(BTreeMap::new()),
        invite: Mutex::new(None),
        paused_all: Mutex::new(false),
        wake: Mutex::new(None),
        machine: Mutex::new(Machine::default()),
        update: Mutex::new(None),
        updates: Mutex::new(UpdatePrefs::load(&pair::config_dir())),
    });
    let for_instance = shared.clone();
    tauri::Builder::default()
        // A second launch (a link clicked while the app runs) hands its link to the first.
        .plugin(tauri_plugin_single_instance::init(move |app, argv, _cwd| {
            take_links(app, &for_instance, argv.into_iter().skip(1).collect());
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        .manage(shared.clone())
        .invoke_handler(tauri::generate_handler![
            get_snapshot, take_invite, inspect, connect, set_paused, pause_all, set_big, fetch, sync_now,
            disconnect, move_local, resolve, open_in, machine_link, machine_set, machine_forget, update_now, open_argus, open_full_disk_settings, check_updates, set_auto_update, set_language, machine_browser, logins_set, login_add, login_remove, login_forget_always, logins_import
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            #[cfg(any(windows, target_os = "linux"))]
            {
                // Installers register the scheme; this makes a copy run from anywhere answer too.
                let _ = app.deep_link().register_all();
            }
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                take_links(&handle, &shared, urls.into_iter().map(|u| u.to_string()).collect());
            }
            let on_link = shared.clone();
            let h2 = handle.clone();
            app.deep_link().on_open_url(move |event| {
                take_links(&h2, &on_link, event.urls().into_iter().map(|u| u.to_string()).collect());
            });

            let language = shared.updates.lock().unwrap().language.clone();
            words::choose(&language);
            let menu = tray_menu(app)?;
            let tray_shared = shared.clone();
            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("argus-bridge")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "open" => show(app),
                    "argus" => {
                        if let Some(sh) = app.try_state::<Arc<Shared>>() {
                            let _ = open_argus_window(app, &sh);
                        }
                    }
                    "now" => tray_shared.nudge(),
                    "check" => {
                        let (a, sh) = (app.clone(), tray_shared.clone());
                        tauri::async_runtime::spawn(async move { let _ = look_for_update(&a, &sh, true).await; });
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        show(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            if std::env::args().any(|a| a == "--hidden") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }

            let (tx, rx) = mpsc::channel();
            *shared.wake.lock().unwrap() = Some(tx);
            let worker_shared = shared.clone();
            let machine_handle = handle.clone();
            std::thread::spawn(move || worker(handle, worker_shared, rx));
            // This computer's connection to Argus, as it was left: on, it connects again.
            start_machine(&machine_handle, &shared);
            // A newer version, looked for at start and every six hours.
            let (up_handle, up_shared) = (machine_handle.clone(), shared.clone());
            std::thread::spawn(move || loop {
                let _ = tauri::async_runtime::block_on(look_for_update(&up_handle, &up_shared, false));
                std::thread::sleep(UPDATE_EVERY);
            });
            Ok(())
        })
        // Closing the window keeps it syncing, in the menu bar / notification area.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("argus-bridge could not start");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the window is drawn from never waits on itself: 0.1.10 locked the update settings twice in
    /// one expression and hung at its first drawing. Run on a thread with a deadline, so a deadlock
    /// fails the test instead of hanging it.
    #[test]
    fn the_snapshot_never_hangs() {
        let dir = std::env::temp_dir().join(format!("argus-bridge-snapshot-{}", std::process::id()));
        let shared = Arc::new(Shared {
            dir: dir.clone(),
            status: Mutex::new(BTreeMap::new()),
            invite: Mutex::new(None),
            paused_all: Mutex::new(false),
            wake: Mutex::new(None),
            machine: Mutex::new(Machine::default()),
            update: Mutex::new(Some("9.9.9".into())),
            updates: Mutex::new(UpdatePrefs { auto: true, later: None, language: "it".into() }),
        });
        let (tx, rx) = mpsc::channel();
        let sh = shared.clone();
        std::thread::spawn(move || {
            let snap = snapshot(&sh);
            let _ = tx.send((snap.auto_update, snap.language, snap.update));
        });
        let got = rx.recv_timeout(Duration::from_secs(5)).expect("snapshot() hung: a lock taken twice?");
        assert_eq!(got, (true, "it".to_string(), Some("9.9.9".to_string())));
        let _ = std::fs::remove_dir_all(dir);
    }
}
