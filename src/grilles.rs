/// Module grilles — sauvegarde centralisée des grilles chordZIC.
///
/// Les grilles sont stockées comme de vrais fichiers JSON dans
/// `~/ChordZIC/grilles/` (dossier créé au besoin, visible par
/// l'utilisateur, backupable, transférable).
///
/// Routes :
/// - `POST   /save`          → écrit/écrase une grille `{...}` avec un champ `name`
/// - `GET    /grilles`       → liste TOUTES les grilles (contenu complet), triée par date desc
/// - `DELETE /grilles/<nom>` → supprime la grille
///
/// Le nom est sanitisé (caractères alphanumériques/unicode, `_`, `-`, espaces)
/// avant d'être utilisé comme nom de fichier : aucun path traversal possible.
use axum::{
    extract::Path,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Dossier de stockage : ~/ChordZIC/grilles/
pub fn grilles_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join("ChordZIC").join("grilles")
}

fn ensure_dir() -> std::io::Result<()> {
    fs::create_dir_all(grilles_dir())
}

/// Sanitise un nom de grille en nom de fichier sûr.
/// Tous les caractères non alphanumériques (sauf `_` et `-`) deviennent `_`,
/// les suites de `_` sont réduites, les `_` de bord sont retirés.
fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // Réduire les suites de '_' et retirer les bords
    let mut out = String::with_capacity(cleaned.len());
    let mut prev_underscore = false;
    for c in cleaned.chars() {
        if c == '_' {
            if prev_underscore {
                continue;
            }
            prev_underscore = true;
        } else {
            prev_underscore = false;
        }
        out.push(c);
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "grille".to_string()
    } else {
        out
    }
}

/// Sécurise un nom reçu dans l'URL (Path) : même sanitisation, mais on
/// refuse aussi les noms qui contiendraient un séparateur de chemin.
fn secure_path_name(name: &str) -> Option<String> {
    let safe = sanitize_name(name);
    if safe.contains('/') || safe.contains('\\') || safe.contains("..") {
        return None;
    }
    Some(safe)
}

// ─── POST /save ──────────────────────────────────────────────────────────
/// Écrit ou écrase une grille. Le body est l'objet JSON complet
/// (format `chordJAVA-grille` v3 + `name` + `savedAt`).
pub async fn save_grille(Json(body): Json<Value>) -> impl IntoResponse {
    if !body.is_object() {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "body JSON objet attendu"})));
    }

    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("grille").to_string();
    let safe = sanitize_name(&name);
    if safe.contains('/') || safe.contains("..") {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "nom invalide"})));
    }

    if let Err(e) = ensure_dir() {
        eprintln!("[grilles] création du dossier impossible : {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": "dossier inaccessible"})));
    }

    let mut doc = body;
    // Toujours stocker le nom original (affichage) dans le fichier
    doc["name"] = json!(name);
    if doc.get("savedAt").and_then(|v| v.as_str()).is_none() {
        doc["savedAt"] = json!(iso_now());
    }

    let path = grilles_dir().join(format!("{safe}.json"));
    let content = match serde_json::to_string_pretty(&doc) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[grilles] sérialisation impossible : {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": "sérialisation"})));
        }
    };
    if let Err(e) = fs::write(&path, content) {
        eprintln!("[grilles] écriture de {} impossible : {e}", path.display());
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": "écriture impossible"})));
    }

    println!("[grilles] 💾 sauvegardée : {}", path.display());
    (StatusCode::OK, Json(json!({"ok": true, "file": safe})))
}

// ─── GET /grilles ────────────────────────────────────────────────────────
/// Liste toutes les grilles (contenu complet), triée par date décroissante.
pub async fn list_grilles() -> impl IntoResponse {
    let mut out: Vec<Value> = Vec::new();

    if let Ok(entries) = fs::read_dir(grilles_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let file = entry.file_name().to_string_lossy().to_string();
            let mtime = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            let mut obj: Value = fs::read_to_string(&path)
                .ok()
                .and_then(|c| serde_json::from_str(&c).ok())
                .unwrap_or(Value::Null);

            if !obj.is_object() {
                continue;
            }
            if obj.get("name").and_then(|v| v.as_str()).is_none() {
                obj["name"] = json!(file.trim_end_matches(".json"));
            }
            obj["file"] = json!(file);
            obj["date"] = json!(mtime);
            out.push(obj);
        }
    }

    // Tri : plus récente d'abord
    out.sort_by(|a, b| {
        b["date"].as_i64().unwrap_or(0).cmp(&a["date"].as_i64().unwrap_or(0))
    });

    (StatusCode::OK, Json(json!(out)))
}

// ─── DELETE /grilles/<nom> ───────────────────────────────────────────────
/// Supprime une grille (par nom de fichier sanitisé).
pub async fn delete_grille(Path(name): Path<String>) -> impl IntoResponse {
    let Some(safe) = secure_path_name(&name) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "nom invalide"})));
    };
    let path = grilles_dir().join(format!("{safe}.json"));

    if !path.exists() {
        return (StatusCode::NOT_FOUND, Json(json!({"ok": false, "error": "grille introuvable"})));
    }
    match fs::remove_file(&path) {
        Ok(()) => {
            println!("[grilles] 🗑️ supprimée : {}", path.display());
            (StatusCode::OK, Json(json!({"ok": true})))
        }
        Err(e) => {
            eprintln!("[grilles] suppression de {} impossible : {e}", path.display());
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": "suppression impossible"})))
        }
    }
}

/// Horodatage ISO-8601 local (approximatif : UTC + offset non géré ici).
fn iso_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Format lisible en UTC (le frontend affiche l'heure locale du navigateur)
    let days = secs / 86400;
    let (y, m, d) = civil_from_days(days as i64);
    let (hh, mm, ss) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Conversion jours → date civile (algorithme de Howard Hinnant).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
