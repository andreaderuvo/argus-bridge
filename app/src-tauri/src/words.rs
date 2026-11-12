//! The few words the Rust side says itself — the tray's menu and tooltip — in the system's
//! language. Everything else is said by the window (ui/words.js); errors travel to it as
//! `{"say": template, "values": {…}}` (see `say`) so it can put them in the same language.

use std::collections::BTreeMap;

/// The language chosen in the window (Settings at its foot), or "" to follow the system's.
static CHOSEN: std::sync::RwLock<String> = std::sync::RwLock::new(String::new());

pub fn choose(lang: &str) {
    *CHOSEN.write().unwrap() = lang.to_string();
}

fn lang() -> String {
    let chosen = CHOSEN.read().unwrap().clone();
    if !chosen.is_empty() {
        return chosen;
    }
    sys_locale::get_locale().unwrap_or_default().chars().take(2).collect::<String>().to_lowercase()
}

pub fn tr(key: &str) -> String {
    let table: &[(&str, &str, &str, &str)] = &[
        // English, Italiano, Español, Français
        ("Open argus-bridge", "Apri argus-bridge", "Abrir argus-bridge", "Ouvrir argus-bridge"),
        ("Sync now", "Sincronizza ora", "Sincronizar ahora", "Synchroniser maintenant"),
        ("Quit", "Esci", "Salir", "Quitter"),
        ("Open Argus", "Apri Argus", "Abrir Argus", "Ouvrir Argus"),
        ("Check for updates…", "Controlla aggiornamenti…", "Buscar actualizaciones…", "Rechercher des mises à jour…"),
        ("A new version of argus-bridge", "Una nuova versione di argus-bridge", "Una nueva versión de argus-bridge", "Une nouvelle version d’argus-bridge"),
        ("argus-bridge {version} is out (you have {current}). Update now? It restarts by itself; your folders and this computer's connection carry on.",
         "È uscita argus-bridge {version} (hai la {current}). Aggiornare ora? Si riavvia da sola; le cartelle e la connessione di questo computer riprendono come prima.",
         "Ha salido argus-bridge {version} (tienes la {current}). ¿Actualizar ahora? Se reinicia sola; las carpetas y la conexión de este equipo siguen como antes.",
         "argus-bridge {version} est sortie (vous avez la {current}). Mettre à jour maintenant ? Elle redémarre seule ; vos dossiers et la connexion de cet ordinateur reprennent comme avant."),
        ("Update and restart", "Aggiorna e riavvia", "Actualizar y reiniciar", "Mettre à jour et redémarrer"),
        ("Later", "Più tardi", "Más tarde", "Plus tard"),
        ("You have the newest version ({version}).", "Hai già la versione più recente ({version}).", "Ya tienes la versión más reciente ({version}).", "Vous avez déjà la version la plus récente ({version})."),
        ("Sign an agent in?", "Far entrare un agente?", "¿Iniciar sesión de un agente?", "Connecter un agent ?"),
        ("An agent asks to be signed in to {site} as {account}, with the login kept in {from}. It never sees the password.",
         "Un agente chiede di entrare su {site} come {account}, con il login salvato in {from}. Non vede mai la password.",
         "Un agente pide iniciar sesión en {site} como {account}, con el acceso guardado en {from}. Nunca ve la contraseña.",
         "Un agent demande à se connecter à {site} en tant que {account}, avec l’identifiant gardé dans {from}. Il ne voit jamais le mot de passe."),
        ("Allow once", "Consenti una volta", "Permitir una vez", "Autoriser une fois"),
        ("Always for this site", "Sempre per questo sito", "Siempre en este sitio", "Toujours pour ce site"),
        ("No", "No", "No", "Non"),
        ("{n} logins are now in the keychain. The exported file still holds every password in the clear: move it to the trash?",
         "{n} login sono ora nel portachiavi. Il file esportato contiene ancora tutte le password in chiaro: lo sposto nel cestino?",
         "{n} accesos están ahora en el llavero. El archivo exportado aún contiene todas las contraseñas en claro: ¿lo muevo a la papelera?",
         "{n} identifiants sont maintenant dans le trousseau. Le fichier exporté contient encore tous les mots de passe en clair : le mettre à la corbeille ?"),
        ("Move it to the trash", "Spostalo nel cestino", "Moverlo a la papelera", "Le mettre à la corbeille"),
        ("Keep it", "Tienilo", "Conservarlo", "Le garder"),
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
        ("An agent is using your browser", "Un agente sta usando il tuo browser", "Un agente está usando tu navegador", "Un agent utilise votre navigateur"),
        ("Agents stopped: the browser is no longer lent. Switch Browser on to lend it again.", "Agenti fermati: il browser non è più prestato. Riaccendi Browser per prestarlo di nuovo.", "Agentes detenidos: el navegador ya no se presta. Activa Navegador para prestarlo de nuevo.", "Agents arrêtés : le navigateur n’est plus prêté. Réactivez Navigateur pour le prêter à nouveau."),
        ("{name} saved in Downloads", "{name} salvato in Download", "{name} guardado en Descargas", "{name} enregistré dans Téléchargements"),
        ("{name} could not be downloaded", "{name} non è stato scaricato", "{name} no se pudo descargar", "{name} n’a pas pu être téléchargé"),
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
