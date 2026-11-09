//! A Chrome left on the browser profile by an earlier run (2026-11-09, issue #15 of Argus): the new one
//! handed over to it and ended at once, and every agent's connection was refused until someone quit
//! Chrome by hand. Now it is taken over when its port is known, and ended and replaced when it is not.
//! Needs a Chromium: `ARGUS_SYNC_CHROME`, else Playwright's cache; skipped without one.

use argus_sync_core::computer::start_chrome;
use std::path::PathBuf;

fn chromium() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ARGUS_SYNC_CHROME") {
        return Some(PathBuf::from(p));
    }
    let cache = dirs::home_dir()?.join(".cache/ms-playwright");
    let mut found: Vec<PathBuf> = std::fs::read_dir(cache).ok()?.flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("chromium-"))
        .map(|e| e.path().join("chrome-linux64/chrome")).filter(|p| p.exists()).collect();
    found.sort();
    found.pop()
}

fn answers(port: u16) -> bool {
    std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
}

#[test]
fn a_chrome_left_on_the_profile_is_taken_over_or_replaced() {
    let Some(exe) = chromium() else {
        eprintln!("SKIPPED: no Chromium (set ARGUS_SYNC_CHROME)");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let profile = tmp.path().join("browser-profile");
    let extra = vec!["--headless=new".to_string(), "--no-sandbox".to_string()];

    // A run that ends without closing its Chrome.
    let first = start_chrome(&exe, &profile, &extra).expect("a first Chrome");
    let (port, path) = (first.port, first.ws_path.clone());
    std::mem::forget(first);
    assert!(answers(port));
    assert!(std::fs::symlink_metadata(profile.join("SingletonLock")).is_ok(), "Chrome holds the profile");

    // Its port still written on the profile: taken over, the same browser.
    let second = start_chrome(&exe, &profile, &extra).expect("taken over");
    assert_eq!((second.port, second.ws_path.clone()), (port, path), "the same Chrome");
    assert!(second.child.is_none());
    std::mem::forget(second);

    // Its port lost (an earlier version deleted the file before starting): it is ended, and a new one starts.
    std::fs::remove_file(profile.join("DevToolsActivePort")).unwrap();
    let third = start_chrome(&exe, &profile, &extra).expect("replaced, not refused");
    assert!(third.child.is_some() && answers(third.port));
    assert!(!answers(port) || third.port == port, "the one left behind is gone");
    drop(third);
}
