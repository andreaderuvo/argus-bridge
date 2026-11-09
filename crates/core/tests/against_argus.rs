//! The engine against a real Argus: started from its source (`ARGUS_SRC`, else the worktree beside
//! this repository), on a throwaway tmux socket, with a temporary root, config and HOME — never the
//! machine's own server. Skipped, loudly, when there is no Argus source to start.

use argus_sync_core::engine::{Engine, Event, Settings};
use argus_sync_core::local;
use argus_sync_core::plan::Issue;
use argus_sync_core::remote::Remote;
use unicode_normalization::UnicodeNormalization;
use std::collections::BTreeSet;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const MASTER: &str = "mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm";

struct Argus {
    child: Child,
    url: String,
    root: PathBuf,
    conf: PathBuf,
    _tmp: tempfile::TempDir,
}

impl Drop for Argus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn source() -> Option<PathBuf> {
    let p = std::env::var_os("ARGUS_SRC").map(PathBuf::from).unwrap_or_else(|| {
        dirs::home_dir().unwrap_or_default().join("argus-wt/argus-sync")
    });
    p.join("app/syncapi.py").exists().then_some(p)
}

fn start() -> Option<Argus> {
    let Some(src) = source() else {
        eprintln!("SKIPPED: no Argus with app/syncapi.py (set ARGUS_SRC)");
        return None;
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    let home = tmp.path().join("home");
    let conf = tmp.path().join("config");
    for d in [&root, &home, &conf] {
        fs::create_dir_all(d).unwrap();
    }
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    fs::write(conf.join("config.yaml"), format!(
        "token: {MASTER}\nroots: [{}]\nlisten: 127.0.0.1:{port}\nallow_write: true\ncheck_releases: false\n",
        root.display()
    )).unwrap();
    let child = Command::new("python3")
        .args(["-m", "app.main", "--config"]).arg(conf.join("config.yaml"))
        .args(["--socket", &format!("argus-sync-t-{}-{port}", std::process::id()), "--listen", &format!("127.0.0.1:{port}")])
        .current_dir(&src)
        .env("HOME", &home)
        .env_remove("TMUX").env_remove("TMUX_PANE")
        .env("ARGUS_OLD_TOOLS", "off")
        .stdout(fs::File::create(conf.join("argus.out")).unwrap()).stderr(fs::File::create(conf.join("argus.log")).unwrap())
        .spawn().expect("python3");
    let url = format!("http://127.0.0.1:{port}");
    let t0 = Instant::now();
    while ureq::get(&format!("{url}/")).call().is_err() {
        // What it said, when it did not start: a missing package reads very differently from a slow runner.
        assert!(t0.elapsed() < Duration::from_secs(120), "Argus did not start:\n{}\n{}",
                fs::read_to_string(conf.join("argus.out")).unwrap_or_default(),
                fs::read_to_string(conf.join("argus.log")).unwrap_or_default());
        std::thread::sleep(Duration::from_millis(200));
    }
    Some(Argus { child, url, root, conf, _tmp: tmp })
}

impl Argus {
    /// A sync key for `folder` (under the root), as the page makes one.
    fn key(&self, folder: &str, write: bool) -> String {
        let r: serde_json::Value = ureq::post(&format!("{}/api/sync/keys", self.url))
            .set("authorization", &format!("Bearer {MASTER}"))
            .send_json(serde_json::json!({"path": self.root.join(folder), "base": self.url, "write": write}))
            .unwrap().into_json().unwrap();
        r["token"].as_str().unwrap().to_string()
    }
}

fn engine(a: &Argus, key: &str, here: &Path, big: u64) -> Engine {
    Engine::new(Remote::new(&a.url, key), Settings {
        root: here.to_path_buf(),
        state_file: here.parent().unwrap().join("state.json"),
        fallback_trash: here.parent().unwrap().join("trash"),
        big,
        wanted: BTreeSet::new(),
        host: "laptop".into(),
    })
}

fn round(e: &mut Engine) -> (Vec<Event>, Vec<Issue>) {
    let r = e.sync_once(&mut |_| {}).expect("a round");
    (r.events, r.issues)
}

fn mtime(p: &Path) -> i64 {
    local::mtime_ns(&fs::metadata(p).unwrap())
}

fn later(p: &Path, text: &str) {
    // A write that is surely later than the last one, whatever the clock's resolution.
    std::thread::sleep(Duration::from_millis(20));
    fs::write(p, text).unwrap();
}

#[test]
fn two_way_sync_against_a_real_argus() {
    let Some(a) = start() else { return };
    // Never the real trash: the freedesktop trash lives under XDG_DATA_HOME.
    let work = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_DATA_HOME", work.path().join("xdg"));
    let srv = a.root.join("data");
    fs::create_dir_all(srv.join("sub")).unwrap();
    fs::write(srv.join("a.txt"), "alpha").unwrap();
    fs::write(srv.join("sub/b.txt"), "beta").unwrap();
    fs::write(srv.join("reads.fastq"), vec![b'A'; 5000]).unwrap();
    fs::write(srv.join(".DS_Store"), "litter").unwrap();
    let here = work.path().join("mirror");
    let key = a.key("data", true);
    let mut e = engine(&a, &key, &here, 1000);

    // 1. The first round brings everything but the big file, which waits to be asked for.
    let (_, issues) = round(&mut e);
    assert_eq!(fs::read_to_string(here.join("a.txt")).unwrap(), "alpha");
    assert_eq!(fs::read_to_string(here.join("sub/b.txt")).unwrap(), "beta");
    assert_eq!(mtime(&here.join("a.txt")), mtime(&srv.join("a.txt")), "the server's time, kept");
    assert!(!here.join("reads.fastq").exists() && !here.join(".DS_Store").exists());
    assert!(issues.iter().any(|i| matches!(i, Issue::Big { size: 5000, .. })), "{issues:?}");
    let (events, _) = round(&mut e);
    assert!(events.is_empty(), "nothing more to do: {events:?}");

    // 2. Changed here → there; changed there → here; a new folder with a file → there.
    later(&here.join("a.txt"), "alpha, edited on the laptop");
    later(&srv.join("sub/b.txt"), "beta, rewritten by a job");
    fs::create_dir_all(here.join("figures")).unwrap();
    fs::write(here.join("figures/plot.svg"), "<svg/>").unwrap();
    round(&mut e);
    assert_eq!(fs::read_to_string(srv.join("a.txt")).unwrap(), "alpha, edited on the laptop");
    assert_eq!(fs::read_to_string(here.join("sub/b.txt")).unwrap(), "beta, rewritten by a job");
    assert_eq!(fs::read_to_string(srv.join("figures/plot.svg")).unwrap(), "<svg/>");
    assert_eq!(mtime(&srv.join("a.txt")), mtime(&here.join("a.txt")), "the laptop's time, kept there");

    // 3. Both changed: both kept, on both sides.
    later(&here.join("a.txt"), "mine");
    later(&srv.join("a.txt"), "theirs");
    let (events, _) = round(&mut e);
    let copy = events.iter().find_map(|ev| match ev { Event::Conflict { copy, .. } => Some(copy.clone()), _ => None }).expect("a conflict");
    assert!(copy.starts_with("a (conflict from laptop "), "{copy}");
    assert_eq!(fs::read_to_string(here.join("a.txt")).unwrap(), "theirs");
    assert_eq!(fs::read_to_string(here.join(&copy)).unwrap(), "mine");
    assert_eq!(fs::read_to_string(srv.join(&copy)).unwrap(), "mine", "the copy reaches the server too");

    // 4. Deleted here → into the server's trash; deleted there → into this computer's trash.
    fs::remove_file(here.join("figures/plot.svg")).unwrap();
    fs::remove_file(srv.join("sub/b.txt")).unwrap();
    round(&mut e);
    assert!(!srv.join("figures/plot.svg").exists());
    let trashed: Vec<_> = walk(&a.conf.join("sync-trash"));
    assert!(trashed.iter().any(|p| p.ends_with("figures/plot.svg")), "{trashed:?}");
    assert!(!here.join("sub/b.txt").exists());
    round(&mut e);
    let (events, _) = round(&mut e);
    assert!(events.is_empty(), "settled: {events:?}");

    // 5. A name written by a Mac (NFD) and the same name written on Linux (NFC) are one file.
    let nfc = "relazione_\u{00e8}.txt";
    let nfd = "relazione_e\u{0300}.txt";
    fs::write(srv.join(nfc), "same").unwrap();
    fs::write(here.join(nfd), "same").unwrap();
    round(&mut e);
    let (events, _) = round(&mut e);
    assert!(events.is_empty(), "{events:?}");
    assert!(!here.join(nfc).exists() || cfg!(target_os = "macos"), "not fetched as a second file");
    // Counted, not asked with exists(): a Mac's file system answers yes for either form of a name
    // (it does not tell NFC from NFD), so only the listing says whether there are one or two.
    let same: Vec<_> = fs::read_dir(&srv).unwrap().filter_map(|d| d.ok()).map(|d| d.file_name().to_string_lossy().to_string())
        .filter(|n| n.nfc().collect::<String>() == nfc).collect();
    assert_eq!(same.len(), 1, "not uploaded as a second file: {same:?}");

    // 6. Asked for, the big file comes.
    e.settings.wanted.insert("reads.fastq".into());
    round(&mut e);
    assert_eq!(fs::metadata(here.join("reads.fastq")).unwrap().len(), 5000);
}

#[test]
fn a_read_only_key_only_brings_things_down() {
    let Some(a) = start() else { return };
    let work = tempfile::tempdir().unwrap();
    let srv = a.root.join("ro");
    fs::create_dir_all(&srv).unwrap();
    fs::write(srv.join("report.pdf"), "pdf").unwrap();
    let here = work.path().join("mirror");
    let key = a.key("ro", false);
    let mut e = engine(&a, &key, &here, 0);
    round(&mut e);
    later(&here.join("report.pdf"), "scribbled on");
    fs::write(here.join("mine.txt"), "only here").unwrap();
    round(&mut e);
    assert_eq!(fs::read_to_string(here.join("report.pdf")).unwrap(), "pdf", "the server's copy comes back");
    assert_eq!(fs::read_to_string(srv.join("report.pdf")).unwrap(), "pdf");
    assert!(!srv.join("mine.txt").exists() && here.join("mine.txt").exists());
}

#[test]
fn a_key_taken_back_says_so() {
    let Some(a) = start() else { return };
    let work = tempfile::tempdir().unwrap();
    fs::create_dir_all(a.root.join("gone")).unwrap();
    let mut e = engine(&a, "as_not-a-real-key", &work.path().join("m"), 0);
    let err = e.sync_once(&mut |_| {}).unwrap_err();
    assert!(err.to_string().contains("no longer accepts this key"), "{err}");
}

fn walk(dir: &Path) -> Vec<String> {
    walkdir::WalkDir::new(dir).into_iter().flatten().map(|e| e.path().to_string_lossy().into_owned()).collect()
}

#[test]
fn a_folder_moved_or_emptied_here_never_empties_the_server() {
    // 2026-11-09: a folder synced, then moved on the computer; the engine still on the old place made it
    // again, empty, and deleted all 14,503 files on the server. Now: gone → an error, nothing deleted;
    // emptied or a quarter gone at once → held and said; one file deleted → deleted, as before.
    let Some(a) = start() else { return };
    let work = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_DATA_HOME", work.path().join("xdg"));
    let srv = a.root.join("valeferri");
    fs::create_dir_all(srv.join("originali")).unwrap();
    for i in 0..40 {
        fs::write(srv.join(format!("originali/f{i}.txt")), format!("file {i}")).unwrap();
    }
    let here = work.path().join("valeferri3");
    let key = a.key("valeferri", true);
    let mut e = engine(&a, &key, &here, 0);
    round(&mut e);
    assert_eq!(fs::read_dir(here.join("originali")).unwrap().count(), 40);
    let on_server = || fs::read_dir(srv.join("originali")).unwrap().count();

    // Moved away (renamed in the Finder, or a disk not mounted): an error, and the folder not made again.
    fs::rename(&here, work.path().join("vale ferri 3")).unwrap();
    let err = e.sync_once(&mut |_| {}).unwrap_err();
    assert!(err.to_string().contains("gone"), "{err}");
    assert!(!here.exists(), "the old place is not made again, empty");
    assert_eq!(on_server(), 40, "nothing deleted on the server");
    fs::rename(work.path().join("vale ferri 3"), &here).unwrap();

    // Emptied: held, said, nothing deleted.
    fs::remove_dir_all(here.join("originali")).unwrap();
    let (_, issues) = round(&mut e);
    assert!(issues.iter().any(|i| matches!(i, Issue::HeldDeletes { count: 40 })), "{issues:?}");
    assert_eq!(on_server(), 40);
    // Still held next round; and the files come back from the server only if asked — here they are put back.
    round(&mut e);
    assert_eq!(on_server(), 40);
    fs::create_dir_all(here.join("originali")).unwrap();
    for i in 0..40 {
        fs::write(here.join(format!("originali/f{i}.txt")), format!("file {i}")).unwrap();
    }
    round(&mut e);
    assert_eq!(on_server(), 40);

    // A quarter gone at once: held too.
    for i in 0..25 {
        fs::remove_file(here.join(format!("originali/f{i}.txt"))).unwrap();
    }
    let (_, issues) = round(&mut e);
    assert!(issues.iter().any(|i| matches!(i, Issue::HeldDeletes { .. })), "{issues:?}");
    assert_eq!(on_server(), 40);

    // One file deleted by hand: deleted on the server, as always (into its trash).
    for i in 0..25 {
        fs::write(here.join(format!("originali/f{i}.txt")), format!("file {i}")).unwrap();
    }
    round(&mut e);
    fs::remove_file(here.join("originali/f39.txt")).unwrap();
    round(&mut e);
    assert_eq!(on_server(), 39);
}
