//! Signing an agent in with a saved login, end to end: a password manager's export imported, a real
//! sign-in page served here, a real Chromium, the fields filled — and never on another site.
//! Needs a Chromium: `ARGUS_SYNC_CHROME`, else Playwright's cache; the browser part is skipped without one.

use argus_sync_core::computer::start_chrome;
use argus_sync_core::logins::{self, Memory, Vault};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

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

const CHROME_EXPORT: &str = "name,url,username,password,note\n\
Site,https://login.example.com/login,sam@example.com,s3cret-site,\n\
Local,http://127.0.0.1/,tester,pa55-local,\n\
App,android://hash@com.app,someone,nope,\n";

const APPLE_EXPORT: &str = "Title,URL,Username,Password,Notes,OTPAuth\n\
Lab,https://lab.example.org,me@lab,lab-pass,,otpauth://totp/Lab?secret=JBSWY3DPEHPK3PXP\n";

#[test]
fn exports_of_chrome_and_apple_passwords_are_imported_and_found_by_site() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = Vault::with_store(tmp.path(), Box::new(Memory::default()));
    std::fs::write(tmp.path().join("chrome.csv"), CHROME_EXPORT).unwrap();
    std::fs::write(tmp.path().join("apple.csv"), APPLE_EXPORT).unwrap();
    assert_eq!(vault.import_csv(&tmp.path().join("chrome.csv")).unwrap(), 2, "the android:// one is skipped");
    assert_eq!(vault.import_csv(&tmp.path().join("apple.csv")).unwrap(), 1);

    let book = std::fs::read_to_string(tmp.path().join("logins.json")).unwrap();
    assert!(!book.contains("s3cret") && !book.contains("lab-pass") && !book.contains("JBSWY"), "no secret in the list");

    let found = logins::accounts_for(&vault, "login.example.com");
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].site.as_str(), found[0].username.as_str()), ("login.example.com", "sam@example.com"));
    assert!(logins::login_for(&vault, "example.com.evil.com", None).is_err());
    let lab = logins::login_for(&vault, "lab.example.org", None).unwrap();
    assert!(lab.account.has_otp && lab.otp.is_some());
    assert!(logins::login_for(&vault, "nowhere.example", None).unwrap_err().contains("no saved login"));

    // Two accounts on one site: the agent is told the names, and picks one.
    vault.add("lab.example.org", "other@lab", "x", None).unwrap();
    let both = logins::login_for(&vault, "lab.example.org", None).unwrap_err();
    assert!(both.contains("me@lab") && both.contains("other@lab"), "{both}");
    assert_eq!(logins::login_for(&vault, "lab.example.org", Some("other@lab")).unwrap().account.username, "other@lab");
    vault.remove("lab.example.org", "other@lab").unwrap();
    assert_eq!(logins::accounts_for(&vault, "lab.example.org").len(), 1);
}

/// A sign-in page on this machine: `/one` asks for everything at once, `/two` only the account (a
/// two-step sign-in), `/code` a one-time code. What is submitted is not needed: the fields are read back.
fn serve() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]);
            let body = if head.starts_with("GET /two") {
                r#"<form><input type="email" name="identifier" placeholder="Email"><button>Next</button></form>"#
            } else if head.starts_with("GET /code") {
                r#"<form><input type="text" autocomplete="one-time-code" name="otp"><button>Verify</button></form>"#
            } else {
                r#"<form><input type="text" name="username"><input type="password" name="password"><button>Sign in</button></form>"#
            };
            let _ = write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), body);
        }
    });
    port
}

#[test]
fn a_saved_login_is_filled_in_the_page_and_only_on_its_site() {
    let Some(exe) = chromium() else {
        eprintln!("SKIPPED: no Chromium (set ARGUS_SYNC_CHROME)");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let vault = Vault::with_store(tmp.path(), Box::new(Memory::default()));
    vault.add("127.0.0.1", "tester", "pa55-local", Some("JBSWY3DPEHPK3PXP")).unwrap();
    vault.add("localhost", "elsewhere", "not-here", None).unwrap();
    let web = serve();
    let chrome = start_chrome(&exe, &tmp.path().join("profile"), &["--headless=new".into(), "--no-sandbox".into()]).unwrap();

    let open = |path: &str| -> String {
        // A browser just started may refuse its first new tab for a moment: asked again, briefly.
        let url = format!("http://127.0.0.1:{}/json/new?http://127.0.0.1:{web}{path}", chrome.port);
        let t: serde_json::Value = (0..20).find_map(|_| match ureq::put(&url).call() {
            Ok(r) => r.into_json().ok(),
            Err(_) => { std::thread::sleep(Duration::from_millis(250)); None }
        }).expect("a new tab");
        let id = t["id"].as_str().unwrap().to_string();
        // Loaded: the page says its host (before, a fill is refused — the page could be anywhere).
        for _ in 0..50 {
            if logins::look(chrome.port, &id).map(|(host, ..)| !host.is_empty()).unwrap_or(false) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        id
    };
    let read = |target: &str, sel: &str| -> String {
        let list: serde_json::Value = ureq::get(&format!("http://127.0.0.1:{}/json/list", chrome.port)).call().unwrap().into_json().unwrap();
        let url = list.as_array().unwrap().iter().find(|t| t["id"] == target).unwrap()["webSocketDebuggerUrl"].as_str().unwrap().to_string();
        let (mut ws, _) = tungstenite::connect(url.as_str()).unwrap();
        let q = serde_json::json!({"id": 1, "method": "Runtime.evaluate", "params": {"expression": format!("document.querySelector('{sel}').value"), "returnByValue": true}});
        ws.send(tungstenite::Message::Text(q.to_string().into())).unwrap();
        loop {
            if let Ok(tungstenite::Message::Text(t)) = ws.read() {
                let v: serde_json::Value = serde_json::from_str(t.as_str()).unwrap();
                if v["id"] == 1 {
                    return v["result"]["result"]["value"].as_str().unwrap_or_default().to_string();
                }
            }
        }
    };

    // Everything at once: the account and the password typed in; the agent told which, not with what.
    let one = open("/one");
    let login = logins::login_for(&vault, "127.0.0.1", None).unwrap();
    let filled = logins::fill(chrome.port, &one, &login).unwrap();
    assert!(filled.username && filled.password && !filled.otp, "{filled:?}");
    assert_eq!((read(&one, "[name=username]"), read(&one, "[name=password]")), ("tester".to_string(), "pa55-local".to_string()));
    assert!(!format!("{filled:?}").contains("pa55"));

    // A two-step sign-in: the account only, now.
    let two = open("/two");
    let filled = logins::fill(chrome.port, &two, &login).unwrap();
    assert!(filled.username && !filled.password);
    assert_eq!(read(&two, "[name=identifier]"), "tester");

    // A one-time code, made from the key kept with the login.
    let code = open("/code");
    let filled = logins::fill(chrome.port, &code, &login).unwrap();
    assert!(filled.otp);
    let typed = read(&code, "[name=otp]");
    assert!(typed.len() == 6 && typed.chars().all(|c| c.is_ascii_digit()), "{typed}");

    // Never on another site: the login of "localhost" is refused on 127.0.0.1, and nothing is typed.
    let other = open("/one");
    let wrong = logins::login_for(&vault, "localhost", None).unwrap();
    let said = logins::fill(chrome.port, &other, &wrong).unwrap_err();
    assert!(said.contains("nothing was filled"), "{said}");
    assert_eq!(read(&other, "[name=password]"), "");
}
