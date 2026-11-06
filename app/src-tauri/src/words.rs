//! The few words the Rust side says itself — the tray's menu and tooltip — in the system's
//! language. Everything else is said by the window (ui/words.js); errors travel to it as
//! `{"say": template, "values": {…}}` (see `say`) so it can put them in the same language.

use std::collections::BTreeMap;

fn lang() -> String {
    sys_locale::get_locale().unwrap_or_default().chars().take(2).collect::<String>().to_lowercase()
}

pub fn tr(key: &str) -> String {
    let table: &[(&str, &str, &str, &str)] = &[
        // English, Italiano, Español, Français
        ("Open argus-bridge", "Apri argus-bridge", "Abrir argus-bridge", "Ouvrir argus-bridge"),
        ("Sync now", "Sincronizza ora", "Sincronizar ahora", "Synchroniser maintenant"),
        ("Quit", "Esci", "Salir", "Quitter"),
        ("Open Argus", "Apri Argus", "Abrir Argus", "Ouvrir Argus"),
        ("Updated to {version}", "Aggiornato alla {version}", "Actualizado a la {version}", "Mis à jour vers la {version}"),
        ("{session} is waiting for you", "{session} ti aspetta", "{session} te espera", "{session} vous attend"),
        ("{session} failed", "{session} non è riuscito", "{session} falló", "{session} a échoué"),
        ("{session} has finished", "{session} ha finito", "{session} ha terminado", "{session} a terminé"),
        ("argus-bridge — nothing connected", "argus-bridge — nessuna cartella collegata", "argus-bridge — ninguna carpeta conectada", "argus-bridge — aucun dossier connecté"),
        ("argus-bridge — paused", "argus-bridge — in pausa", "argus-bridge — en pausa", "argus-bridge — en pause"),
        ("argus-bridge — {n} conflict(s) to look at", "argus-bridge — {n} conflitti da guardare", "argus-bridge — {n} conflictos por revisar", "argus-bridge — {n} conflit(s) à regarder"),
        ("argus-bridge — {n} folder(s) cannot reach the server", "argus-bridge — {n} cartelle non raggiungono il server", "argus-bridge — {n} carpetas no llegan al servidor", "argus-bridge — {n} dossier(s) n’atteignent pas le serveur"),
        ("argus-bridge — syncing…", "argus-bridge — sincronizzazione…", "argus-bridge — sincronizando…", "argus-bridge — synchronisation…"),
        ("argus-bridge — up to date", "argus-bridge — tutto aggiornato", "argus-bridge — todo al día", "argus-bridge — à jour"),
    ];
    let col = match lang().as_str() { "it" => 1, "es" => 2, "fr" => 3, _ => 0 };
    table.iter().find(|row| row.0 == key).map(|row| [row.0, row.1, row.2, row.3][col]).unwrap_or(key).to_string()
}

pub fn tr_n(key: &str, n: usize) -> String {
    tr(key).replace("{n}", &n.to_string())
}

/// A message for the window to say in the person's language: the English template is the key.
pub fn say(template: &str, values: &[(&str, String)]) -> String {
    let values: BTreeMap<&str, &String> = values.iter().map(|(k, v)| (*k, v)).collect();
    serde_json::json!({ "say": template, "values": values }).to_string()
}
