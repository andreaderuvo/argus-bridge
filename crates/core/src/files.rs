//! Files on this computer, for Argus: a folder listed or searched, a file brought to the server, a file
//! written here. "Find file x in my Downloads and bring it in" (2026-11-12) could be searched for with a
//! command, never brought: a command answers in text, cut at a size. Under the Actions switch — running a
//! command here already reaches every file, so this is no new power, only one that carries bytes.
//!
//! One channel per request (`open {kind: "file", do, path, …}`):
//! - `list`: `{path, match?, deep?}` → one message, JSON `{path, entries: [{name, path, size, mtime, dir}], more}`.
//! - `get`: `{path}` → a message JSON `{name, size}`, then the bytes in base64 messages, then `close` code 0.
//! - `put`: `{path, replace?}` → the server sends the bytes in base64 messages and `{end: true}`; written to a
//!   part file and renamed into place, then `close` code 0. An existing file is kept unless `replace`.

use crate::computer::{In, Out};
use base64::Engine;
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CHUNK: usize = 192 * 1024;
const LIST_MAX: usize = 500;
const DEEP_MAX: usize = 50_000;
const PUT_IDLE: Duration = Duration::from_secs(60);

/// `~` and `~/…` as this computer's home; anything else as given (absolute, or under the home).
pub fn resolve(said: &str) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    let said = said.trim();
    if said.is_empty() || said == "~" {
        home
    } else if let Some(rest) = said.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(said).is_absolute() {
        PathBuf::from(said)
    } else {
        home.join(said)
    }
}

fn mtime(meta: &fs::Metadata) -> f64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// What is in a folder — or under it, `deep` — whose name contains `wanted` (any case): newest first.
pub fn list(path: &Path, wanted: &str, deep: bool) -> Result<Value, String> {
    let meta = fs::metadata(path).map_err(|e| format!("{}: {}", path.display(), plain(&e)))?;
    if !meta.is_dir() {
        return Err(format!("{} is a file, not a folder", path.display()));
    }
    let wanted = wanted.trim().to_lowercase();
    let mut found = Vec::new();
    let mut seen = 0usize;
    let walker = walkdir::WalkDir::new(path).min_depth(1).max_depth(if deep { 12 } else { 1 }).follow_links(false);
    for entry in walker.into_iter().filter_map(Result::ok) {
        seen += 1;
        if seen > DEEP_MAX {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || (!wanted.is_empty() && !name.to_lowercase().contains(&wanted)) {
            continue;
        }
        let Ok(m) = entry.metadata() else { continue };
        found.push(json!({"name": name, "path": entry.path().to_string_lossy(), "size": if m.is_dir() { 0 } else { m.len() },
                          "mtime": mtime(&m), "dir": m.is_dir()}));
    }
    found.sort_by(|a, b| b["mtime"].as_f64().unwrap_or(0.0).partial_cmp(&a["mtime"].as_f64().unwrap_or(0.0)).unwrap());
    let more = found.len() > LIST_MAX || seen > DEEP_MAX;
    found.truncate(LIST_MAX);
    Ok(json!({"path": path.to_string_lossy(), "entries": found, "more": more}))
}

/// The error in words, with what usually fixes it on a Mac.
fn plain(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "not found".into(),
        std::io::ErrorKind::PermissionDenied => if cfg!(target_os = "macos") {
            "not allowed — macOS protects this folder: give argus-bridge Full Disk Access (This computer, in the app)".into()
        } else {
            "not allowed".into()
        },
        _ => e.to_string(),
    }
}

pub fn serve(ch: u64, open: &Value, inbox: Receiver<In>, out: Out) {
    let path = resolve(open.get("path").and_then(Value::as_str).unwrap_or(""));
    let close = |why: String| {
        let _ = out.send(json!({"op": "close", "ch": ch, "why": why}));
    };
    match open.get("do").and_then(Value::as_str).unwrap_or("") {
        "list" => {
            let wanted = open.get("match").and_then(Value::as_str).unwrap_or("");
            match list(&path, wanted, open.get("deep").and_then(Value::as_bool).unwrap_or(false)) {
                Ok(v) => {
                    let _ = out.send(json!({"op": "opened", "ch": ch}));
                    let _ = out.send(json!({"op": "msg", "ch": ch, "data": v.to_string()}));
                    let _ = out.send(json!({"op": "close", "ch": ch, "code": 0}));
                }
                Err(why) => close(why),
            }
        }
        "get" => {
            let mut file = match fs::File::open(&path) {
                Ok(f) => f,
                Err(e) => return close(format!("{}: {}", path.display(), plain(&e))),
            };
            let size = match file.metadata() {
                Ok(m) if m.is_dir() => return close(format!("{} is a folder: bring its files one by one", path.display())),
                Ok(m) => m.len(),
                Err(e) => return close(plain(&e)),
            };
            let _ = out.send(json!({"op": "opened", "ch": ch}));
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let _ = out.send(json!({"op": "msg", "ch": ch, "data": json!({"name": name, "size": size}).to_string()}));
            let mut buf = vec![0u8; CHUNK];
            loop {
                if matches!(inbox.try_recv(), Ok(In::Close)) {
                    return;
                }
                match file.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&buf[..n]);
                        if out.send(json!({"op": "msg", "ch": ch, "b64": b64})).is_err() {
                            return;
                        }
                    }
                    Err(e) => return close(plain(&e)),
                }
            }
            let _ = out.send(json!({"op": "close", "ch": ch, "code": 0}));
        }
        "put" => {
            let replace = open.get("replace").and_then(Value::as_bool).unwrap_or(false);
            if path.exists() && !replace {
                return close(format!("{} is there already — say replace to write over it", path.display()));
            }
            let Some(folder) = path.parent().filter(|f| f.is_dir()) else {
                return close(format!("no folder {} on this computer", path.parent().map(|f| f.display().to_string()).unwrap_or_default()));
            };
            let part = folder.join(format!(".{}.argus-part", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
            let mut file = match fs::File::create(&part) {
                Ok(f) => f,
                Err(e) => return close(format!("{}: {}", folder.display(), plain(&e))),
            };
            let _ = out.send(json!({"op": "opened", "ch": ch}));
            loop {
                match inbox.recv_timeout(PUT_IDLE) {
                    Ok(In::Frame(f)) => {
                        if f.get("end").and_then(Value::as_bool) == Some(true) {
                            break;
                        }
                        let bytes = f.get("b64").and_then(Value::as_str)
                            .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok()).unwrap_or_default();
                        if let Err(e) = file.write_all(&bytes) {
                            let _ = fs::remove_file(&part);
                            return close(plain(&e));
                        }
                    }
                    Ok(In::Close) | Err(_) => {
                        let _ = fs::remove_file(&part);
                        return close("stopped before the end: nothing written".into());
                    }
                }
            }
            drop(file);
            match fs::rename(&part, &path) {
                Ok(()) => {
                    let _ = out.send(json!({"op": "close", "ch": ch, "code": 0}));
                }
                Err(e) => {
                    let _ = fs::remove_file(&part);
                    close(plain(&e));
                }
            }
        }
        _ => close("files: list, get or put".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_listed_newest_first_and_searched_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("report-old.pdf"), "a").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        fs::write(tmp.path().join("Report-new.pdf"), "bb").unwrap();
        fs::write(tmp.path().join(".hidden"), "").unwrap();
        fs::create_dir(tmp.path().join("deeper")).unwrap();
        fs::write(tmp.path().join("deeper/report-deep.txt"), "c").unwrap();
        let v = list(tmp.path(), "REPORT", false).unwrap();
        let names: Vec<_> = v["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, ["Report-new.pdf", "report-old.pdf"], "newest first, any case, hidden left out, not deeper");
        let deep = list(tmp.path(), "report", true).unwrap();
        assert_eq!(deep["entries"].as_array().unwrap().len(), 3);
        assert!(list(&tmp.path().join("report-old.pdf"), "", false).unwrap_err().contains("not a folder"));
    }

    #[test]
    fn home_is_where_a_bare_or_tilde_path_points() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(resolve("~/Downloads"), home.join("Downloads"));
        assert_eq!(resolve("Downloads"), home.join("Downloads"));
        assert_eq!(resolve("/tmp/x"), PathBuf::from("/tmp/x"));
    }

    #[test]
    fn a_file_goes_out_in_pieces_and_comes_in_whole_never_over_another() {
        let tmp = tempfile::tempdir().unwrap();
        let data: Vec<u8> = (0..(CHUNK * 2 + 77)).map(|i| (i % 251) as u8).collect();
        fs::write(tmp.path().join("big.bin"), &data).unwrap();
        // Out: a header, then the bytes.
        let (out, got) = std::sync::mpsc::channel();
        let (_tx, rx) = std::sync::mpsc::channel();
        serve(1, &json!({"do": "get", "path": tmp.path().join("big.bin").to_string_lossy()}), rx, out);
        let frames: Vec<Value> = got.try_iter().collect();
        let head: Value = serde_json::from_str(frames[1]["data"].as_str().unwrap()).unwrap();
        assert_eq!(head["size"], data.len() as u64);
        let back: Vec<u8> = frames.iter().filter_map(|f| f["b64"].as_str())
            .flat_map(|b| base64::engine::general_purpose::STANDARD.decode(b).unwrap()).collect();
        assert_eq!(back, data);
        assert_eq!(frames.last().unwrap()["code"], 0);
        // In: the pieces, then the end; written whole, by its name.
        let (out, got) = std::sync::mpsc::channel();
        let (tx, rx) = std::sync::mpsc::channel();
        let target = tmp.path().join("copy.bin");
        for piece in data.chunks(CHUNK) {
            tx.send(In::Frame(json!({"b64": base64::engine::general_purpose::STANDARD.encode(piece)}))).unwrap();
        }
        tx.send(In::Frame(json!({"end": true}))).unwrap();
        serve(2, &json!({"do": "put", "path": target.to_string_lossy()}), rx, out);
        assert_eq!(got.try_iter().last().unwrap()["code"], 0);
        assert_eq!(fs::read(&target).unwrap(), data);
        // Never over a file that is there, unless asked.
        let (out, got) = std::sync::mpsc::channel();
        let (_tx, rx) = std::sync::mpsc::channel();
        serve(3, &json!({"do": "put", "path": target.to_string_lossy()}), rx, out);
        assert!(got.try_iter().last().unwrap()["why"].as_str().unwrap().contains("there already"));
    }
}
