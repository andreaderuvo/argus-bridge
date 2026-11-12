//! Signing agents in with the person's saved logins — without the agent ever reading them.
//!
//! The state of the art of 2026 (1Password's Secure Agentic Autofill, Bitwarden's MCP server): the
//! secret never enters the model's context; a trusted component on the person's own computer puts it
//! straight into the page; only on the site it belongs to; each use approved by the person; every use
//! written down. This module is that component's core:
//!
//! - **where the logins are**: a vault of this app in the system's keychain (macOS Keychain, Windows
//!   Credential Manager, the Secret Service on Linux), filled by hand or from the CSV every password
//!   manager exports (Chrome, Apple Passwords, Firefox, Bitwarden, 1Password…); and 1Password itself,
//!   read live through its own command line, unlocked by its app.
//! - **which one fits**: a login belongs to a site; it is offered only to that site and its subdomains.
//! - **putting it in**: into the fields of the agent's tab, through the browser's own debugging
//!   connection, after the tab's host is checked once more — the agent is told what was filled, never
//!   with what.
//!
//! What a login holds is a [`Secret`]: it has no `Serialize`, prints as dots, and is only ever given to
//! [`fill`].

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// A password or a one-time-code key: kept, never shown. No `Serialize`, and `Debug` prints dots.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Secret {
        Secret(value.into())
    }
    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("••••")
    }
}

/// What may be said about a login — to the window, to Argus, to an agent: the site, the account's
/// name and where it is kept. Never the password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub site: String,
    pub username: String,
    /// "vault" (this app's, in the system keychain) or "1password".
    pub source: String,
    #[serde(default)]
    pub has_otp: bool,
    /// A name the person gives this login, to tell an agent which one to use ("sign in with XYZ") without
    /// saying the account — and to choose among several accounts on one site. Not secret; unique.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub code: String,
}

/// A login, with its secrets: only built to be filled, and dropped after.
#[derive(Debug, Clone)]
pub struct Login {
    pub account: Account,
    pub password: Secret,
    /// The key one-time codes are made from (TOTP), or a code already made (1Password gives the code).
    pub otp: Option<Otp>,
}

#[derive(Debug, Clone)]
pub enum Otp {
    Key(Secret),
    Code(Secret),
}

// ------------------------------------------------------------------------------------- which site

/// The host a URL or a typed site names, lower-case, without scheme, port, path or "www.".
pub fn site_of(said: &str) -> String {
    let s = said.trim().to_lowercase();
    let s = s.split("://").last().unwrap_or(&s);
    let s = s.split(['/', '?', '#']).next().unwrap_or(s);
    let s = s.rsplit('@').next().unwrap_or(s);
    let s = s.split(':').next().unwrap_or(s);
    s.trim_start_matches("www.").trim_end_matches('.').to_string()
}

/// Whether a login kept for `site` may be used on `host`: the same host, or one of its subdomains —
/// the login of `example.com` on `login.example.com`, never on `example.com.evil.com`.
pub fn host_matches(site: &str, host: &str) -> bool {
    let (site, host) = (site_of(site), site_of(host));
    !site.is_empty() && (host == site || host.ends_with(&format!(".{site}")))
}

// ------------------------------------------------------------------------------------- the vault

const SERVICE: &str = "argus-bridge";

/// Where the secrets are kept: the system's keychain in the app; memory in tests (a keychain is not
/// there on a CI machine, and keyring's own stand-in forgets between two entries).
pub trait Store: Send + Sync {
    fn set(&self, name: &str, value: &str) -> Result<(), String>;
    fn get(&self, name: &str) -> Result<String, String>;
    fn delete(&self, name: &str) -> Result<(), String>;
}

/// The system's keychain: macOS Keychain, Windows Credential Manager, the Secret Service on Linux.
pub struct Keychain;

impl Store for Keychain {
    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        keyring::Entry::new(SERVICE, name).and_then(|e| e.set_password(value)).map_err(|e| e.to_string())
    }
    fn get(&self, name: &str) -> Result<String, String> {
        keyring::Entry::new(SERVICE, name).and_then(|e| e.get_password()).map_err(|e| e.to_string())
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        keyring::Entry::new(SERVICE, name).and_then(|e| e.delete_credential()).map_err(|e| e.to_string())
    }
}

/// In memory, for tests.
#[derive(Default)]
pub struct Memory(std::sync::Mutex<std::collections::HashMap<String, String>>);

impl Store for Memory {
    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(name.into(), value.into());
        Ok(())
    }
    fn get(&self, name: &str) -> Result<String, String> {
        self.0.lock().unwrap().get(name).cloned().ok_or_else(|| "not there".into())
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}

/// This app's own logins: the secrets in the system's keychain, one entry each; the list of what is
/// there (sites and account names, nothing secret) in `logins.json` beside the app's settings, since a
/// keychain cannot be listed.
pub struct Vault {
    dir: PathBuf,
    store: Box<dyn Store>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Book {
    #[serde(default)]
    pub accounts: Vec<Account>,
    /// Agents may be signed in at all (off until switched on, in the app's Browser settings).
    #[serde(default)]
    pub enabled: bool,
    /// Sites where a sign-in is allowed without asking each time ("Always for this site").
    #[serde(default)]
    pub always: Vec<String>,
    /// 1Password looked in too (when its command line is installed and signed in).
    #[serde(default)]
    pub onepassword: bool,
    /// Where the passwords are: "file" (argus-bridge's own sealed file) or "keychain" (the system's). "" is
    /// the default — see `keep_of`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub keep: String,
    /// Sign in without asking, on every site: for a person who automates everything and wants no question
    /// (2026-11-12). Off unless switched on.
    #[serde(default)]
    pub never_ask: bool,
}

/// Where a book's passwords are. Unset: the sealed file on a Mac — the Keychain asks again after every update
/// of an app without an Apple Developer ID, once per login ("I want to automate everything without these
/// popups", 2026-11-12) — and the system's keychain elsewhere, where it does not; but always the keychain for
/// a book that already has logins there from before the choice existed.
pub fn keep_of(book: &Book) -> &'static str {
    match book.keep.as_str() {
        "file" => "file",
        "keychain" => "keychain",
        _ if book.accounts.iter().any(|a| a.source == "vault") => "keychain",
        _ if cfg!(target_os = "macos") => "file",
        _ => "keychain",
    }
}

/// argus-bridge's own file of passwords, sealed (ChaCha20-Poly1305) with a key kept beside it, readable by
/// this user only. It keeps the passwords out of every plain file and backup; it does not keep them from a
/// program running as you, which the Keychain would ask about — the price of never being asked.
pub struct Sealed {
    dir: PathBuf,
}

impl Sealed {
    pub fn new(dir: &Path) -> Sealed {
        Sealed { dir: dir.to_path_buf() }
    }

    fn key(&self) -> Result<chacha20poly1305::Key, String> {
        use chacha20poly1305::KeyInit;
        let path = self.dir.join("logins.key");
        if let Ok(bytes) = fs::read(&path) {
            if bytes.len() == 32 {
                return Ok(*chacha20poly1305::Key::from_slice(&bytes));
            }
            return Err("the key of the logins file does not read".into());
        }
        fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let key = chacha20poly1305::ChaCha20Poly1305::generate_key(&mut chacha20poly1305::aead::OsRng);
        private_write(&path, key.as_slice())?;
        Ok(key)
    }

    fn load(&self) -> Result<std::collections::BTreeMap<String, String>, String> {
        use base64::Engine;
        use chacha20poly1305::{aead::Aead, KeyInit};
        let Ok(raw) = fs::read(self.dir.join("logins.sealed")) else { return Ok(Default::default()) };
        let sealed = base64::engine::general_purpose::STANDARD.decode(raw.trim_ascii()).map_err(|_| "the logins file does not read".to_string())?;
        if sealed.len() < 12 {
            return Err("the logins file does not read".into());
        }
        let cipher = chacha20poly1305::ChaCha20Poly1305::new(&self.key()?);
        let plain = cipher.decrypt(chacha20poly1305::Nonce::from_slice(&sealed[..12]), &sealed[12..])
            .map_err(|_| "the logins file does not open with its key".to_string())?;
        serde_json::from_slice(&plain).map_err(|_| "the logins file does not read".to_string())
    }

    fn save(&self, all: &std::collections::BTreeMap<String, String>) -> Result<(), String> {
        use base64::Engine;
        use chacha20poly1305::{aead::{Aead, AeadCore}, KeyInit};
        let cipher = chacha20poly1305::ChaCha20Poly1305::new(&self.key()?);
        let nonce = chacha20poly1305::ChaCha20Poly1305::generate_nonce(&mut chacha20poly1305::aead::OsRng);
        let plain = serde_json::to_vec(all).map_err(|e| e.to_string())?;
        let mut sealed = nonce.to_vec();
        sealed.extend(cipher.encrypt(&nonce, plain.as_ref()).map_err(|_| "could not seal the logins".to_string())?);
        let tmp = self.dir.join("logins.sealed.tmp");
        private_write(&tmp, base64::engine::general_purpose::STANDARD.encode(sealed).as_bytes())?;
        fs::rename(tmp, self.dir.join("logins.sealed")).map_err(|e| e.to_string())
    }
}

/// Written readable by this user only.
fn private_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options.open(path).and_then(|mut f| f.write_all(bytes)).map_err(|e| e.to_string())
}

impl Store for Sealed {
    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        let mut all = self.load()?;
        all.insert(name.into(), value.into());
        self.save(&all)
    }
    fn get(&self, name: &str) -> Result<String, String> {
        self.load()?.remove(name).ok_or_else(|| "not in the logins file".into())
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        let mut all = self.load()?;
        if all.remove(name).is_some() {
            self.save(&all)?;
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct Kept {
    p: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    t: Option<String>,
}

impl Vault {
    /// Where the person chose (`keep_of`): argus-bridge's sealed file, or the system's keychain.
    pub fn new(dir: &Path) -> Vault {
        let book: Book = fs::read(dir.join("logins.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Vault::with_store(dir, Vault::store_for(dir, keep_of(&book)))
    }

    fn store_for(dir: &Path, keep: &str) -> Box<dyn Store> {
        if keep == "file" { Box::new(Sealed::new(dir)) } else { Box::new(Keychain) }
    }

    /// Every kept password moved to `to` ("file" or "keychain"), then the choice written. Each is written in
    /// the new place before it is deleted from the old; one that cannot be read stops the move, with nothing lost.
    pub fn move_to(&self, to: &str, to_store: &dyn Store) -> Result<usize, String> {
        if to != "file" && to != "keychain" {
            return Err("logins are kept in the file or in the keychain".into());
        }
        let mut book = self.book();
        let mut moved = 0;
        for a in book.accounts.iter().filter(|a| a.source == "vault") {
            let name = Vault::name(&a.site, &a.username);
            let value = self.store.get(&name).map_err(|e| format!("the login of {} on {} could not be read to move it: {e}", a.username, a.site))?;
            to_store.set(&name, &value)?;
            let _ = self.store.delete(&name);
            moved += 1;
        }
        book.keep = to.to_string();
        self.save_book(&book).map_err(|e| e.to_string())?;
        Ok(moved)
    }

    /// The same, from the app: the store of `to` made here.
    pub fn move_logins(dir: &Path, to: &str) -> Result<usize, String> {
        Vault::new(dir).move_to(to, Vault::store_for(dir, to).as_ref())
    }

    pub fn with_store(dir: &Path, store: Box<dyn Store>) -> Vault {
        Vault { dir: dir.to_path_buf(), store }
    }

    fn file(&self) -> PathBuf {
        self.dir.join("logins.json")
    }

    pub fn book(&self) -> Book {
        fs::read(self.file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save_book(&self, book: &Book) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join("logins.json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(book)?)?;
        fs::rename(tmp, self.file())
    }

    fn name(site: &str, username: &str) -> String {
        format!("{}\u{1f}{}", site_of(site), username)
    }

    /// Kept, or replaced when the same site and account are there already.
    pub fn add(&self, site: &str, username: &str, password: &str, otp_key: Option<&str>) -> Result<Account, String> {
        self.add_coded(site, username, password, otp_key, "")
    }

    /// The same, with the code the person gives it ("" for none).
    pub fn add_coded(&self, site: &str, username: &str, password: &str, otp_key: Option<&str>, code: &str) -> Result<Account, String> {
        let site = site_of(site);
        if site.is_empty() || username.trim().is_empty() || password.is_empty() {
            return Err("a login needs a site, an account and a password".into());
        }
        let mut code = self.free_code(code, &site, username.trim())?;
        if code.is_empty() {
            // Kept again (an import, a new password): the code it had stays. Cleared only by set_code.
            code = self.book().accounts.iter().find(|a| a.site == site && a.username == username.trim())
                .map(|a| a.code.clone()).unwrap_or_default();
        }
        let otp_key = otp_key.map(str::trim).filter(|k| !k.is_empty()).map(str::to_string);
        if let Some(k) = &otp_key {
            otp_code(k).map_err(|e| format!("the one-time-code key does not read: {e}"))?;
        }
        let kept = serde_json::to_string(&Kept { p: password.to_string(), t: otp_key.clone() }).map_err(|e| e.to_string())?;
        self.store.set(&Vault::name(&site, username.trim()), &kept)?;
        let account = Account { site, username: username.trim().to_string(), source: "vault".into(), has_otp: otp_key.is_some(), code };
        let mut book = self.book();
        book.accounts.retain(|a| !(a.site == account.site && a.username == account.username));
        book.accounts.push(account.clone());
        book.accounts.sort_by(|a, b| (&a.site, &a.username).cmp(&(&b.site, &b.username)));
        self.save_book(&book).map_err(|e| e.to_string())?;
        Ok(account)
    }

    /// A code that reads (letters, digits, `-`, `_`, `.`, up to 40) and no other login has. "" is no code.
    /// `site` and `username` are the login the code is for: its own code is not "taken".
    fn free_code(&self, code: &str, site: &str, username: &str) -> Result<String, String> {
        let code = code.trim();
        if code.is_empty() {
            return Ok(String::new());
        }
        if code.len() > 40 || !code.chars().all(|c| c.is_alphanumeric() || "-_.".contains(c)) {
            return Err("a code is letters, digits, - _ or . — up to 40, no spaces".into());
        }
        if let Some(other) = self.book().accounts.iter()
            .find(|a| a.code.eq_ignore_ascii_case(code) && !(a.site == site && a.username == username)) {
            return Err(format!("the code {code} is already the login of {} on {}", other.username, other.site));
        }
        Ok(code.to_string())
    }

    /// Give a kept login a code, or change or clear it ("").
    pub fn set_code(&self, site: &str, username: &str, code: &str) -> Result<Account, String> {
        let site = site_of(site);
        let code = self.free_code(code, &site, username)?;
        let mut book = self.book();
        let account = book.accounts.iter_mut().find(|a| a.site == site && a.username == username)
            .ok_or_else(|| format!("no kept login of {username} on {site}"))?;
        account.code = code;
        let out = account.clone();
        self.save_book(&book).map_err(|e| e.to_string())?;
        Ok(out)
    }

    /// A kept login changed in place: its site (the secret moved in the keychain, never typed again) and its
    /// code. A domain account kept for "lab" and used on jenkins.lab.example (2026-11-11): a site given wrong
    /// could only be fixed by deleting the login and typing the password again.
    pub fn edit(&self, site: &str, username: &str, new_site: &str, code: &str) -> Result<Account, String> {
        let (site, new_site) = (site_of(site), site_of(new_site));
        if new_site.is_empty() {
            return Err("a login needs a site".into());
        }
        let mut book = self.book();
        let at = book.accounts.iter().position(|a| a.site == site && a.username == username && a.source == "vault")
            .ok_or_else(|| format!("no kept login of {username} on {site}"))?;
        let code = self.free_code(code, &site, username)?;
        if new_site != site {
            if book.accounts.iter().any(|a| a.site == new_site && a.username == username) {
                return Err(format!("{username} already has a login on {new_site}"));
            }
            let kept = self.store.get(&Vault::name(&site, username))
                .map_err(|e| format!("the keychain did not give the login of {username} on {site}: {e}"))?;
            self.store.set(&Vault::name(&new_site, username), &kept)?;
            let _ = self.store.delete(&Vault::name(&site, username));
        }
        let account = &mut book.accounts[at];
        account.site = new_site;
        account.code = code;
        let out = account.clone();
        book.accounts.sort_by(|a, b| (&a.site, &a.username).cmp(&(&b.site, &b.username)));
        self.save_book(&book).map_err(|e| e.to_string())?;
        Ok(out)
    }

    pub fn remove(&self, site: &str, username: &str) -> Result<(), String> {
        let site = site_of(site);
        let _ = self.store.delete(&Vault::name(&site, username));
        let mut book = self.book();
        book.accounts.retain(|a| !(a.site == site && a.username == username));
        self.save_book(&book).map_err(|e| e.to_string())
    }

    fn get(&self, account: &Account) -> Result<Login, String> {
        let raw = self.store.get(&Vault::name(&account.site, &account.username))
            .map_err(|e| format!("the keychain did not give the login of {} on {}: {e}", account.username, account.site))?;
        let kept: Kept = serde_json::from_str(&raw).map_err(|_| "a login in the keychain does not read".to_string())?;
        Ok(Login { account: account.clone(), password: Secret::new(kept.p), otp: kept.t.map(|t| Otp::Key(Secret::new(t))) })
    }

    /// The logins of a password manager's export: the columns of Chrome, Apple Passwords, Firefox,
    /// Bitwarden and 1Password (url / login_uri / website, username / login_username, password /
    /// login_password, and a one-time-code column when there is one). Returns how many were kept.
    pub fn import_csv(&self, path: &Path) -> Result<usize, String> {
        let mut reader = csv::ReaderBuilder::new().flexible(true).from_path(path).map_err(|e| e.to_string())?;
        let heads: Vec<String> = reader.headers().map_err(|e| e.to_string())?.iter().map(|h| h.trim().to_lowercase()).collect();
        let col = |names: &[&str]| heads.iter().position(|h| names.contains(&h.as_str()));
        let (url, user, pass) = (
            col(&["url", "login_uri", "website", "web site", "origin", "uri", "login url"]),
            col(&["username", "login_username", "user name", "login", "email", "user"]),
            col(&["password", "login_password"]),
        );
        let otp = col(&["totp", "login_totp", "otpauth", "one-time password", "otp", "otp auth"]);
        let (Some(url), Some(user), Some(pass)) = (url, user, pass) else {
            return Err("not a password export: no url, username and password columns".into());
        };
        let mut kept = 0;
        for row in reader.records().flatten() {
            let get = |i: usize| row.get(i).unwrap_or("").to_string();
            let (site, username, password) = (get(url), get(user), get(pass));
            if site_of(&site).is_empty() || username.trim().is_empty() || password.is_empty() || site.starts_with("android://") {
                continue;
            }
            let key = otp.map(get).filter(|k| !k.trim().is_empty());
            if self.add(&site, &username, &password, key.as_deref()).is_ok() {
                kept += 1;
            }
        }
        Ok(kept)
    }
}

// ------------------------------------------------------------------------------------- 1Password

/// 1Password, through its own command line (`op`), unlocked by its app (Touch ID): its logins are never
/// copied anywhere — read when they are filled.
pub mod onepassword {
    use super::*;

    fn op(args: &[&str]) -> Result<String, String> {
        let out = Command::new(exe()).args(args).output().map_err(|e| format!("1Password's command line: {e}"))?;
        if !out.status.success() {
            return Err(format!("1Password: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    /// `op` where it is usually installed: an app started from the Dock has no shell PATH.
    fn exe() -> PathBuf {
        for p in ["/opt/homebrew/bin/op", "/usr/local/bin/op", "/usr/bin/op"] {
            if Path::new(p).is_file() {
                return PathBuf::from(p);
            }
        }
        PathBuf::from("op")
    }

    pub fn installed() -> bool {
        Command::new(exe()).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }

    /// The logins whose website is this host's site, with their 1Password ids.
    pub fn accounts(host: &str) -> Result<Vec<(Account, String)>, String> {
        let items: Value = serde_json::from_str(&op(&["item", "list", "--categories", "Login", "--format", "json"])?)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for item in items.as_array().into_iter().flatten() {
            let urls: Vec<String> = item["urls"].as_array().into_iter().flatten()
                .filter_map(|u| u["href"].as_str().map(str::to_string)).collect();
            if let Some(url) = urls.iter().find(|u| host_matches(u, host)) {
                let id = item["id"].as_str().unwrap_or_default().to_string();
                let username = item["additional_information"].as_str().unwrap_or_default().to_string();
                out.push((Account { site: site_of(url), username, source: "1password".into(), has_otp: false, code: String::new() }, id));
            }
        }
        Ok(out)
    }

    pub fn get(account: &Account, id: &str) -> Result<Login, String> {
        let fields: Value = serde_json::from_str(&op(&["item", "get", id, "--fields", "label=username,label=password", "--reveal", "--format", "json"])?)
            .map_err(|e| e.to_string())?;
        let value = |label: &str| fields.as_array().into_iter().flatten()
            .find(|f| f["label"].as_str() == Some(label)).and_then(|f| f["value"].as_str()).unwrap_or_default().to_string();
        let otp = op(&["item", "get", id, "--otp"]).ok().map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
        let mut account = account.clone();
        if account.username.is_empty() {
            account.username = value("username");
        }
        account.has_otp = otp.is_some();
        Ok(Login { account, password: Secret::new(value("password")), otp: otp.map(|c| Otp::Code(Secret::new(c))) })
    }
}

// ------------------------------------------------------------------------------------- finding one

/// Every login there is for this host, in the sources switched on: what may be shown or asked about.
pub fn accounts_for(vault: &Vault, host: &str) -> Vec<Account> {
    let book = vault.book();
    let mut out: Vec<Account> = book.accounts.iter().filter(|a| host_matches(&a.site, host)).cloned().collect();
    if book.onepassword {
        if let Ok(found) = onepassword::accounts(host) {
            out.extend(found.into_iter().map(|(a, _)| a));
        }
    }
    out
}

/// The login to fill on `host`: the one named by `which` — its code or its account — when given, else the
/// only one there is. A code of a login of another site fills nothing: the page decides the site.
pub fn login_for(vault: &Vault, host: &str, which: Option<&str>) -> Result<Login, String> {
    let book = vault.book();
    if let Some(name) = which.map(str::trim).filter(|n| !n.is_empty()) {
        if let Some(coded) = book.accounts.iter().find(|a| a.code.eq_ignore_ascii_case(name)) {
            if !host_matches(&coded.site, host) {
                return Err(format!("the code {name} is a login for {}, and this page is on {} — nothing was filled. A login's \
                    site covers its subdomains: to use it here, the person changes its site in argus-bridge (Logins, the \
                    pencil on its row) to {}", coded.site, site_of(host), wider(host)));
            }
            return vault.get(coded);
        }
    }
    let mut found: Vec<(Account, Option<String>)> =
        book.accounts.iter().filter(|a| host_matches(&a.site, host)).cloned().map(|a| (a, None)).collect();
    if book.onepassword {
        if let Ok(more) = onepassword::accounts(host) {
            found.extend(more.into_iter().map(|(a, id)| (a, Some(id))));
        }
    }
    if let Some(name) = which.map(str::trim).filter(|n| !n.is_empty()) {
        found.retain(|(a, _)| a.username.eq_ignore_ascii_case(name));
        if found.is_empty() {
            return Err(format!("no saved login for {} is called {name} — neither a code nor an account", site_of(host)));
        }
    }
    match found.len() {
        0 => Err(format!("no saved login for {} — add one in argus-bridge (This computer → Logins), or import your password manager's export", site_of(host))),
        1 => {
            let (account, id) = found.remove(0);
            match id {
                Some(id) => onepassword::get(&account, &id),
                None => vault.get(&account),
            }
        }
        _ => Err(format!("{} logins for {}: say which, by its code or its account — {}", found.len(), site_of(host),
                         found.iter().map(|(a, _)| if a.code.is_empty() { a.username.clone() } else { format!("{} ({})", a.code, a.username) })
                             .collect::<Vec<_>>().join(", "))),
    }
}

/// The site to suggest for a host: its domain without the first name (`jenkins.lab.example` →
/// `lab.example`, which covers the lab's other hosts too), or the host itself when it is short.
fn wider(host: &str) -> String {
    let host = site_of(host);
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() >= 3 && !host.chars().all(|c| c.is_ascii_digit() || c == '.') { parts[1..].join(".") } else { host }
}

/// The current code of a one-time-code key (a base32 secret, or an `otpauth://` link).
pub fn otp_code(key: &str) -> Result<String, String> {
    let key = key.trim();
    let totp = if key.starts_with("otpauth://") {
        totp_rs::TOTP::from_url_unchecked(key).map_err(|e| e.to_string())?
    } else {
        let bytes = totp_rs::Secret::Encoded(key.replace(' ', "").to_uppercase()).to_bytes().map_err(|e| format!("{e:?}"))?;
        totp_rs::TOTP::new_unchecked(totp_rs::Algorithm::SHA1, 6, 1, 30, bytes, None, String::new())
    };
    totp.generate_current().map_err(|e| e.to_string())
}

// ------------------------------------------------------------------------------------- filling

/// What was put into the page — said to the agent: which fields, for which account, from where.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Filled {
    pub host: String,
    pub account: String,
    pub source: String,
    pub username: bool,
    pub password: bool,
    pub otp: bool,
}

/// The fields of a sign-in form that are visible now, marked so they can be filled: the account (an
/// e-mail or user name), the password, a one-time code. A two-step sign-in (Google, Microsoft) shows
/// one of them at a time: each step is filled when it is there.
const FIND: &str = r#"(() => {
  const seen = (e) => { const r = e.getBoundingClientRect(); const s = getComputedStyle(e);
    return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none' && !e.disabled && !e.readOnly; };
  const all = [...document.querySelectorAll('input')].filter(seen);
  const has = (e, words) => words.some((w) => `${e.name} ${e.id} ${e.autocomplete} ${e.placeholder} ${e.getAttribute('aria-label') || ''}`.toLowerCase().includes(w));
  const pass = all.find((e) => e.type === 'password');
  const otp = all.find((e) => e.autocomplete === 'one-time-code' || (['text', 'tel', 'number'].includes(e.type) && has(e, ['otp', 'totp', 'one-time', 'verification code', 'codice', 'code']) && !has(e, ['zip', 'postal', 'promo'])));
  const user = all.find((e) => e !== otp && (e.type === 'email' || (['text', ''].includes(e.type) && (has(e, ['user', 'email', 'e-mail', 'login', 'account', 'identifier', 'utente']) || e.autocomplete === 'username'))));
  for (const e of document.querySelectorAll('[data-argus-fill]')) e.removeAttribute('data-argus-fill');
  if (user) user.dataset.argusFill = 'u';
  if (pass) pass.dataset.argusFill = 'p';
  if (otp && otp !== pass) otp.dataset.argusFill = 'o';
  return { host: location.hostname, user: !!user, pass: !!pass, otp: !!(otp && otp !== pass) };
})()"#;

struct Page {
    ws: tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
    next: u64,
}

impl Page {
    fn open(port: u16, target: &str) -> Result<Page, String> {
        let list: Value = ureq::get(&format!("http://127.0.0.1:{port}/json/list")).timeout(Duration::from_secs(5)).call()
            .map_err(|e| format!("the browser did not answer: {e}"))?.into_json().map_err(|e| e.to_string())?;
        let url = list.as_array().into_iter().flatten()
            .find(|t| t["id"].as_str() == Some(target) && t["type"].as_str() == Some("page"))
            .and_then(|t| t["webSocketDebuggerUrl"].as_str()).ok_or("that tab is not open any more")?.to_string();
        let (ws, _) = tungstenite::connect(url.as_str()).map_err(|e| e.to_string())?;
        Ok(Page { ws, next: 0 })
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        self.ws.send(tungstenite::Message::Text(json!({"id": id, "method": method, "params": params}).to_string().into()))
            .map_err(|e| e.to_string())?;
        loop {
            match self.ws.read().map_err(|e| e.to_string())? {
                tungstenite::Message::Text(t) => {
                    let v: Value = serde_json::from_str(t.as_str()).unwrap_or(Value::Null);
                    if v["id"].as_u64() == Some(id) {
                        if let Some(e) = v.get("error") {
                            return Err(e["message"].as_str().unwrap_or("the browser refused").to_string());
                        }
                        return Ok(v["result"].clone());
                    }
                }
                tungstenite::Message::Close(_) => return Err("the tab closed".into()),
                _ => {}
            }
        }
    }

    fn eval(&mut self, expression: &str) -> Result<Value, String> {
        let r = self.call("Runtime.evaluate", json!({"expression": expression, "returnByValue": true}))?;
        if let Some(e) = r.get("exceptionDetails") {
            return Err(e["text"].as_str().unwrap_or("the page refused").to_string());
        }
        Ok(r["result"]["value"].clone())
    }

    /// Typed as keystrokes into the marked field — what it held replaced — so the page's own handlers
    /// see an ordinary entry.
    fn type_into(&mut self, mark: &str, secret: &Secret) -> Result<(), String> {
        self.eval(&format!(
            "(() => {{ const e = document.querySelector('[data-argus-fill=\"{mark}\"]'); e.focus(); e.select && e.select(); \
             e.value = ''; e.dispatchEvent(new Event('input', {{bubbles: true}})); return true; }})()"
        ))?;
        self.call("Input.insertText", json!({"text": secret.expose()}))?;
        Ok(())
    }
}

/// Look at the agent's tab: its host, and which sign-in fields it shows now — nothing filled.
pub fn look(port: u16, target: &str) -> Result<(String, bool, bool, bool), String> {
    let mut page = Page::open(port, target)?;
    let f = page.eval(FIND)?;
    Ok((f["host"].as_str().unwrap_or_default().to_string(), f["user"] == true, f["pass"] == true, f["otp"] == true))
}

/// Put the login into the sign-in fields the agent's tab shows now. The tab's host is checked again here,
/// right before anything is typed: a login goes only to its own site.
pub fn fill(port: u16, target: &str, login: &Login) -> Result<Filled, String> {
    let mut page = Page::open(port, target)?;
    let f = page.eval(FIND)?;
    let host = f["host"].as_str().unwrap_or_default().to_string();
    if !host_matches(&login.account.site, &host) {
        return Err(format!("this tab is on {host}; the login is for {} — nothing was filled", login.account.site));
    }
    let mut filled = Filled { host, account: login.account.username.clone(), source: login.account.source.clone(),
                              username: false, password: false, otp: false };
    if f["user"] == true {
        page.type_into("u", &Secret::new(login.account.username.clone()))?;
        filled.username = true;
    }
    if f["pass"] == true {
        page.type_into("p", &login.password)?;
        filled.password = true;
    }
    if f["otp"] == true {
        let code = match &login.otp {
            Some(Otp::Code(c)) => Some(c.clone()),
            Some(Otp::Key(k)) => Some(Secret::new(otp_code(k.expose())?)),
            None => None,
        };
        if let Some(code) = code {
            page.type_into("o", &code)?;
            filled.otp = true;
        }
    }
    if !(filled.username || filled.password || filled.otp) {
        return Err("no sign-in field on this page: open the sign-in page first".into());
    }
    Ok(filled)
}

// ------------------------------------------------------------------------------------- asked by Argus

/// What the person decides when an agent asks to be signed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Once,
    Always,
    No,
}

/// Asks the person, on this computer: (the site, the account, where it is kept) → their decision.
pub type AskLogin = std::sync::Arc<dyn Fn(&str, &Account) -> Decision + Send + Sync>;

/// The logins of this computer, as the bridge serves them: where the vault is, and who to ask — the
/// app's dialog; nobody for the command-line client, which signs in only where "Always" was chosen.
#[derive(Clone)]
pub struct Logins {
    pub dir: PathBuf,
    pub ask: Option<AskLogin>,
}

/// A request from Argus on the bridge's `login` channel: `{target, want: "fill"|"list", account?}`.
/// `list` names the accounts kept for the tab's site; `fill` signs the agent in. Answers what may be
/// said — never a secret.
pub fn serve(logins: &Logins, port: u16, frame: &Value) -> Result<Value, String> {
    let vault = Vault::new(&logins.dir);
    serve_with(&vault, logins.ask.as_ref(), port, frame)
}

pub fn serve_with(vault: &Vault, ask: Option<&AskLogin>, port: u16, frame: &Value) -> Result<Value, String> {
    let mut book = vault.book();
    if !book.enabled {
        return Err("signing agents in is off on this computer — the person switches it on in argus-bridge: This computer → Browser settings → Logins".into());
    }
    let target = frame["target"].as_str().filter(|t| !t.is_empty()).ok_or("which tab: the agent's own")?;
    let (host, ..) = look(port, target)?;
    if frame["want"].as_str() == Some("list") {
        return Ok(json!({"host": host, "accounts": accounts_for(vault, &host)}));
    }
    let login = login_for(vault, &host, frame["account"].as_str())?;
    let always = book.never_ask || book.always.iter().any(|s| host_matches(s, &host));
    if !always {
        let Some(ask) = ask else {
            return Err(format!("{} is not among the sites always allowed, and there is nobody here to ask (the command-line client)", site_of(&host)));
        };
        match ask(&site_of(&host), &login.account) {
            Decision::No => return Err("the person said no".into()),
            Decision::Always => {
                book.always.push(login.account.site.clone());
                book.always.sort();
                book.always.dedup();
                vault.save_book(&book).map_err(|e| e.to_string())?;
            }
            Decision::Once => {}
        }
    }
    let filled = fill(port, target, &login)?;
    Ok(serde_json::to_value(filled).unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sealed_file_keeps_passwords_out_of_plain_sight_and_logins_move_between_stores() {
        let tmp = tempfile::tempdir().unwrap();
        let sealed = Sealed::new(tmp.path());
        sealed.set("a.example\u{1f}me", "pa55-word").unwrap();
        assert_eq!(sealed.get("a.example\u{1f}me").unwrap(), "pa55-word");
        let on_disk = std::fs::read_to_string(tmp.path().join("logins.sealed")).unwrap();
        assert!(!on_disk.contains("pa55") && !on_disk.contains("a.example"), "sealed, names too");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for f in ["logins.key", "logins.sealed"] {
                assert_eq!(std::fs::metadata(tmp.path().join(f)).unwrap().permissions().mode() & 0o777, 0o600, "{f}");
            }
        }
        // Another key: it does not open, and says so.
        std::fs::write(tmp.path().join("logins.key"), [7u8; 32]).unwrap();
        assert!(sealed.get("a.example\u{1f}me").unwrap_err().contains("does not open"));

        // From the keychain (here a stand-in) to the file: every login moved, the choice written, nothing left behind.
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::with_store(dir.path(), Box::new(Memory::default()));
        vault.add("one.example", "me", "p1", None).unwrap();
        vault.add("two.example", "me", "p2", Some("JBSWY3DPEHPK3PXP")).unwrap();
        assert_eq!(keep_of(&vault.book()), "keychain", "a book with logins from before stays where they are");
        let file = Sealed::new(dir.path());
        assert_eq!(vault.move_to("file", &file).unwrap(), 2);
        assert_eq!(keep_of(&vault.book()), "file");
        let after = Vault::with_store(dir.path(), Box::new(Sealed::new(dir.path())));
        let two = login_for(&after, "two.example", None).unwrap();
        assert!(two.password.expose() == "p2" && two.otp.is_some());
        assert!(vault.store.get(&Vault::name("one.example", "me")).is_err(), "gone from the old place");
    }

    #[test]
    fn a_new_book_on_a_mac_keeps_its_logins_in_the_file() {
        let fresh = Book::default();
        assert_eq!(keep_of(&fresh), if cfg!(target_os = "macos") { "file" } else { "keychain" });
    }

    #[test]
    fn a_code_names_one_login_among_several_and_only_on_its_site() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::with_store(tmp.path(), Box::new(Memory::default()));
        vault.add_coded("login.example.com", "first@example.com", "p1", None, "XYZ").unwrap();
        vault.add("login.example.com", "second@example.com", "p2", None).unwrap();
        vault.add("login.example.com", "third@example.com", "p3", None).unwrap();
        // Three on one site: by code, by account, and asked which (the codes said) when neither.
        assert_eq!(login_for(&vault, "login.example.com", Some("xyz")).unwrap().account.username, "first@example.com");
        assert_eq!(login_for(&vault, "login.example.com", Some("third@example.com")).unwrap().account.username, "third@example.com");
        let which = login_for(&vault, "login.example.com", None).unwrap_err();
        assert!(which.contains("XYZ (first@example.com)") && which.contains("second@example.com"), "{which}");
        assert!(login_for(&vault, "login.example.com", Some("nobody")).unwrap_err().contains("neither a code nor an account"));
        // The page decides the site: the code of this login fills nothing on another one.
        assert!(login_for(&vault, "other.example.org", Some("XYZ")).unwrap_err().contains("nothing was filled"));
        // Unique, readable, kept when the login is kept again, changed or cleared on purpose.
        assert!(vault.add_coded("login.example.com", "second@example.com", "p2", None, "xyz").unwrap_err().contains("already"));
        assert!(vault.set_code("login.example.com", "second@example.com", "two words").is_err());
        vault.add("login.example.com", "first@example.com", "new", None).unwrap();
        assert_eq!(vault.book().accounts.iter().find(|a| a.username == "first@example.com").unwrap().code, "XYZ");
        assert_eq!(vault.set_code("login.example.com", "second@example.com", "LAB-2").unwrap().code, "LAB-2");
        vault.set_code("login.example.com", "first@example.com", "").unwrap();
        assert!(login_for(&vault, "login.example.com", Some("XYZ")).unwrap_err().contains("neither"));
        // A site given wrong is changed in place: the secret moves, the code stays, and the error said how.
        vault.add_coded("lab", "domain-user", "dp", None, "LAB-ACCOUNT").unwrap();
        let said = login_for(&vault, "jenkins.lab.example", Some("LAB-ACCOUNT")).unwrap_err();
        assert!(said.contains("to lab.example"), "{said}");
        vault.edit("lab", "domain-user", "lab.example", "LAB-ACCOUNT").unwrap();
        let moved = login_for(&vault, "jenkins.lab.example", Some("LAB-ACCOUNT")).unwrap();
        assert_eq!((moved.account.site.as_str(), moved.password.expose()), ("lab.example", "dp"));
        assert!(login_for(&vault, "wiki.lab.example", Some("LAB-ACCOUNT")).is_ok(), "the domain's other hosts too");
        assert!(vault.edit("lab.example", "domain-user", "", "").is_err());
        // Not secret, so in the list; the passwords never are.
        let book = std::fs::read_to_string(tmp.path().join("logins.json")).unwrap();
        assert!(book.contains("LAB-2") && !book.contains("p2"));
    }

    #[test]
    fn a_login_goes_to_its_site_and_its_subdomains_only() {
        assert!(host_matches("example.com", "login.example.com"));
        assert!(host_matches("https://www.example.com/login", "example.com"));
        assert!(!host_matches("example.com", "example.com.evil.com"));
        assert!(!host_matches("example.com", "notexample.com"));
        assert!(!host_matches("", "anything"));
        assert_eq!(site_of("https://user@Accounts.Google.com:443/v3/signin?x=1"), "accounts.google.com");
    }

    #[test]
    fn a_secret_never_prints() {
        let s = Secret::new("hunter2");
        assert_eq!(format!("{s:?}"), "••••");
        let login = Login { account: Account { site: "a.b".into(), username: "me".into(), source: "vault".into(), has_otp: false, code: String::new() },
                            password: s, otp: Some(Otp::Code(Secret::new("123456"))) };
        let said = format!("{login:?}");
        assert!(!said.contains("hunter2") && !said.contains("123456"), "{said}");
    }

    #[test]
    fn one_time_codes_from_a_key_or_a_link() {
        let code = otp_code("JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        assert_eq!(otp_code("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP").unwrap(), code);
        assert!(otp_code("not base32 !!").is_err());
    }
}
