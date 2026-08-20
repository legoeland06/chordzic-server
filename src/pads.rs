//! pads.rs — banque de samples des 64 pads (simulation Ableton Push 3).
//!
//! Chaque pad déclenche un échantillon audio (wav / mp3 / ogg / flac / m4a /
//! aiff) importé par l'utilisateur : à chaque appui, le son s'ARRÊTE et se
//! REDÉCLENCHE sans délai (retrigger — comportement drum machine).
//!
//! L'utilisateur choisit le chemin de LECTURE : navigateur (Web Audio) ou
//! serveur — le backend stocke et sert les fichiers (dossier `$PADS_DIR` ou
//! `~/samples/pads/`) et peut les JOUER via `ffplay` (tous formats, sortie
//! audio du PC) : `POST /pad-trigger` (retrigger par fichier) et
//! `POST /pad-stop` (coupe toutes les lectures serveur).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Mutex;

/// Taille maximale d'un sample importé (25 Mo — les MP3 longs sont lourds).
pub const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

/// Extensions audio acceptées (le navigateur décode via Web Audio).
pub const ALLOWED_EXTS: [&str; 6] = ["wav", "mp3", "ogg", "flac", "m4a", "aiff"];

/// Dossier des samples de pads : `$PADS_DIR` ou `~/samples/pads/`.
pub fn pads_dir() -> String {
    if let Ok(d) = std::env::var("PADS_DIR") {
        if !d.is_empty() {
            return d;
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    format!("{}/samples/pads", home)
}

/// Info d'un sample de pad (pour la liste).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PadInfo {
    pub name: String,
    pub size: u64,
    pub ext: String,
}

/// Liste les samples de pads disponibles (triés par nom).
pub fn list() -> Vec<PadInfo> {
    let dir_str = pads_dir();
    let dir = Path::new(&dir_str);
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(ext) = path.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase()) else {
                continue;
            };
            if !ALLOWED_EXTS.contains(&ext.as_str()) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()).map(String::from) else {
                continue;
            };
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(PadInfo { name, size, ext });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Enregistre un upload : nom unique `pad_<timestamp>_<aléa>.<ext>`.
/// Retourne le nom du fichier (ou une erreur). Crée le dossier si besoin.
pub fn save(data: &[u8], ext: &str) -> Result<String, String> {
    if data.is_empty() {
        return Err("fichier vide".into());
    }
    if data.len() > MAX_UPLOAD_BYTES {
        return Err(format!("fichier trop gros (max {} Mo)", MAX_UPLOAD_BYTES / 1024 / 1024));
    }
    let ext = ext.to_lowercase();
    if !ALLOWED_EXTS.contains(&ext.as_str()) {
        return Err(format!(
            "extension « {} » non supportée (acceptées : {})",
            ext,
            ALLOWED_EXTS.join(", ")
        ));
    }
    let dir_str = pads_dir();
    let dir = Path::new(&dir_str);
    std::fs::create_dir_all(dir).map_err(|e| format!("dossier pads illisible : {e}"))?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let rand: u32 = std::process::id().wrapping_mul(ts as u32).wrapping_add(ts as u32 / 7);
    let name = format!("pad_{}_{}.{}", ts, rand % 10000, ext);
    let path = dir.join(&name);
    std::fs::write(&path, data).map_err(|e| format!("écriture impossible : {e}"))?;
    Ok(name)
}

/// Chemin sécurisé d'un sample par nom (refuse toute traversée de dossier).
pub fn path_for(name: &str) -> Option<PathBuf> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || !name.starts_with("pad_")
    {
        return None;
    }
    Some(Path::new(&pads_dir()).join(name))
}

/// Content-Type selon l'extension.
pub fn content_type(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "aiff" => "audio/aiff",
        _ => "application/octet-stream",
    }
}

// ── Lecture côté SERVEUR (ffplay) ──────────────────────────────────────

/// Erreurs de déclenchement d'une lecture serveur.
#[derive(Debug, PartialEq, Eq)]
pub enum PadPlayError {
    /// Nom de fichier invalide (traversée, mauvais préfixe…).
    BadName,
    /// Fichier présent côté navigateur mais absent du serveur.
    NotFound,
    /// ffplay introuvable ou refusé de démarrer.
    Spawn(String),
}

/// Args ffplay : sans fenêtre, auto-exit, volume 0-100.
pub fn ffplay_args(volume: u8) -> Vec<String> {
    vec![
        "-nodisp".to_string(),
        "-autoexit".to_string(),
        "-loglevel".to_string(),
        "quiet".to_string(),
        "-volume".to_string(),
        volume.min(100).to_string(),
    ]
}

/// Déclenche la lecture d'un sample de pad via ffplay — RETRIGGER : le
/// process précédent du MÊME fichier est tué avant le nouveau spawn.
/// `players` : map fichier → process en cours (état partagé du serveur).
pub fn trigger_pad(file: &str, volume: u8, players: &Mutex<HashMap<String, Child>>) -> Result<(), PadPlayError> {
    let Some(path) = path_for(file) else { return Err(PadPlayError::BadName); };
    if !path.is_file() {
        return Err(PadPlayError::NotFound);
    }
    let mut guard = players.lock().unwrap();
    if let Some(mut old) = guard.remove(file) {
        let _ = old.kill();
    }
    let mut cmd = Command::new("ffplay");
    cmd.args(ffplay_args(volume));
    cmd.arg(&path);
    match cmd.spawn() {
        Ok(child) => {
            guard.insert(file.to_string(), child);
            Ok(())
        }
        Err(e) => Err(PadPlayError::Spawn(e.to_string())),
    }
}

/// Arrête TOUTES les lectures serveur en cours (bouton ■ Stop).
pub fn stop_all_pads(players: &Mutex<HashMap<String, Child>>) {
    let mut guard = players.lock().unwrap();
    for (_, mut c) in guard.drain() {
        let _ = c.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffplay_args_volume_borne() {
        let a = ffplay_args(80);
        assert_eq!(a[0], "-nodisp");
        assert!(a.iter().any(|s| s == "-volume"));
        assert!(a.iter().any(|s| s == "80"));
        assert!(ffplay_args(200).iter().any(|s| s == "100")); // borné
        assert!(ffplay_args(0).iter().any(|s| s == "0"));
    }

    #[test]
    fn trigger_pad_valide_nom_et_existence() {
        let players = Mutex::new(HashMap::new());
        // Nom invalide → BadName (aucun spawn)
        assert_eq!(trigger_pad("autre.wav", 100, &players), Err(PadPlayError::BadName));
        assert_eq!(trigger_pad("pad_1/../../x", 100, &players), Err(PadPlayError::BadName));
        // Nom valide mais fichier absent → NotFound
        assert_eq!(trigger_pad("pad_zzz_inexistant.wav", 100, &players), Err(PadPlayError::NotFound));
    }

    #[test]
    fn extensions_acceptees_et_refusees() {
        assert!(ALLOWED_EXTS.contains(&"wav"));
        assert!(ALLOWED_EXTS.contains(&"mp3"));
        assert!(!ALLOWED_EXTS.contains(&"exe"));
        assert!(!ALLOWED_EXTS.contains(&"mid"));
    }

    #[test]
    fn save_refuse_vide_trop_gros_et_mauvaise_extension() {
        assert!(save(&[], "wav").is_err());
        assert!(save(&vec![0u8; MAX_UPLOAD_BYTES + 1], "wav").is_err());
        assert!(save(b"data", "exe").is_err());
        assert!(save(b"data", "MP3").is_ok()); // extension normalisée
    }

    #[test]
    fn path_for_refuse_la_traversee() {
        assert!(path_for("pad_1.wav").is_some());
        assert!(path_for("pad_1.mp3").is_some());
        assert!(path_for("../secret.wav").is_none());
        assert!(path_for("pad_1/../../x").is_none());
        assert!(path_for("autre.wav").is_none()); // doit commencer par pad_
        assert!(path_for("").is_none());
        assert!(path_for("pad_..wav").is_none()); // « .. » dans le nom
    }

    #[test]
    fn content_type_par_extension() {
        assert_eq!(content_type("wav"), "audio/wav");
        assert_eq!(content_type("MP3"), "audio/mpeg");
        assert_eq!(content_type("flac"), "audio/flac");
        assert_eq!(content_type("inconnu"), "application/octet-stream");
    }
}
