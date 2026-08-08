//! Inventaire des samples drums (~/samples/drums/).
//!
//! Depuis v2.6.1, la LECTURE des boucles se fait UNIQUEMENT en mode Navig
//! (côté navigateur, Web Audio) — le backend ne fait que :
//!  - lister les samples disponibles (GET /samples-list)
//!  - servir les fichiers (GET /sample-file)
//! La convention de nommage est stricte : `<nom>_<tempo>.wav` (ex. snap5_160.wav).

use std::collections::HashMap;
use std::path::Path;

/// Dossier des samples drums : `$SAMPLES_DIR` ou `~/samples/drums/`.
pub fn drum_dir() -> String {
    if let Ok(d) = std::env::var("SAMPLES_DIR") {
        if !d.is_empty() {
            return d;
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    format!("{}/samples/drums", home)
}

/// Liste les samples disponibles groupés par tempo :
/// `{ "160": ["snap5", "snap6"], ... }` — clés = nom sans `_<tempo>.wav`.
/// Appelé par GET /samples-list (le frontend reconstruit le nom complet
/// `<clé>_<tempo>.wav` pour GET /sample-file).
pub fn get_available() -> serde_json::Value {
    let mut map: HashMap<u16, Vec<String>> = HashMap::new();
    let dir_str = drum_dir();
    let dir = Path::new(&dir_str);
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("wav") {
                continue;
            }
            let Some(fname) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            // <nom>_<tempo> — le suffixe après le dernier '_' doit être un BPM
            let Some(us) = fname.rfind('_') else { continue };
            let Ok(bpm) = fname[us + 1..].parse::<u16>() else { continue };
            map.entry(bpm).or_default().push(fname[..us].to_string());
        }
    }

    let mut tempos: Vec<u16> = map.keys().copied().collect();
    tempos.sort();
    let mut out = serde_json::Map::new();
    for t in tempos {
        let mut names = map.remove(&t).unwrap_or_default();
        names.sort();
        out.insert(
            t.to_string(),
            serde_json::Value::Array(
                names
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
    }
    serde_json::Value::Object(out)
}
