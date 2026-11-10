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


/// What a page sees: not a browser "controlled by automated software" (Argus issue #16 — Google refused
/// to sign in the lent browser, which said navigator.webdriver = true).
#[test]
fn a_page_does_not_see_an_automated_browser() {
    let Some(exe) = chromium() else {
        eprintln!("SKIPPED: no Chromium (set ARGUS_SYNC_CHROME)");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    // As the app starts it: the person's settings, the default ones here.
    let mut extra = argus_sync_core::computer::BrowserSettings::default().args();
    extra.extend(["--headless=new".to_string(), "--no-sandbox".to_string()]);
    let chrome = start_chrome(&exe, &tmp.path().join("profile"), &extra).expect("a Chrome");
    let list: serde_json::Value = ureq::get(&format!("http://127.0.0.1:{}/json/list", chrome.port)).call().unwrap().into_json().unwrap();
    let page = list.as_array().unwrap().iter().find(|t| t["type"] == "page").expect("a page")["webSocketDebuggerUrl"].as_str().unwrap().to_string();
    let (mut ws, _) = tungstenite::connect(page.as_str()).expect("the page");
    ws.send(tungstenite::Message::Text(r#"{"id":1,"method":"Runtime.evaluate","params":{"expression":"navigator.webdriver","returnByValue":true}}"#.into())).unwrap();
    let answer: serde_json::Value = loop {
        if let Ok(tungstenite::Message::Text(t)) = ws.read() {
            let v: serde_json::Value = serde_json::from_str(t.as_str()).unwrap();
            if v["id"] == 1 { break v; }
        }
    };
    assert_eq!(answer["result"]["result"]["value"], serde_json::json!(false), "{answer}");
}
