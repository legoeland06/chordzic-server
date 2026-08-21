//! live_instrument.rs — Moteur live multi-source pour le monitoring du pianiste.
//!
//! Chemin : notes MIDI (clavier Roland, entrée gérée par `live_input`) →
//! moteur choisi → son. Trois sources :
//!
//! - **Thru** : les notes reviennent au Roland (son GM interne) — DÉFAUT.
//! - **Vst3** : plugin VST3 (Surge XT) → sortie audio USB du Roland
//!   (device ALSA « roland », plug → hw:3,0) → haut-parleurs du Roland.
//! - **Fluid** : FluidSynth (SoundFont GM du serveur) — son PC.
//!
//! Config VST3 validée avec le Roland FP-60X (spike live, 2026-08-20) :
//! **44 100 Hz + buffer 256** — le Roland (carte 3) ne supporte QUE
//! 44,1 kHz en S24_3LE → le device « roland » de ~/.asoundrc (type plug)
//! fait la conversion de format. PipeWire ne monitor pas la carte 3 →
//! ouvrable en direct par ALSA/cpal.
//!
//! # Threads
//!
//! `cpal::Stream` n'est PAS `Send` (pointeur ALSA) → il vit sur un thread
//! moteur dédié (« vst3-live »), qui reçoit les commandes (start / stop /
//! set_preset) par canal et garde le stream + l'hôte VST3 dans sa boucle.
//! Le plugin (`Plugin` est `Send`) est déposé dans l'état partagé : le
//! callback audio (thread ALSA) le verrouille en `try_lock` (zéro blocage
//! du thread RT), les routes HTTP en `lock` normal pour changer de preset.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use vst3_host::audio::AudioBuffers;
use vst3_host::midi::{MidiChannel, MidiEvent};
use vst3_host::{Plugin, Vst3Host};

/// Fréquence du moteur live — seule rate supportée par le Roland en USB.
pub const LIVE_RATE: u32 = 44_100;
/// Taille de buffer validée : 128 → XRUN, 256+ refusé par le plug si rate ≠ 44,1.
pub const LIVE_BUFFER: usize = 256;

/// Commande envoyée au thread moteur (chaque commande attend sa réponse).
enum Cmd {
    Start { preset: String, reply: Sender<Result<(), String>> },
    SetPreset { preset: String, reply: Sender<Result<(), String>> },
    Stop { reply: Sender<()> },
}

/// Source du moteur live : où partent les notes du pianiste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveSource {
    /// Thru MIDI : les notes reviennent au Roland (son GM interne) — DÉFAUT.
    Thru,
    /// Plugin VST3 (Surge XT) → audio USB → haut-parleurs du Roland.
    Vst3,
    /// FluidSynth (SoundFont GM du serveur) — son PC.
    Fluid,
}

impl LiveSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            LiveSource::Thru => "thru",
            LiveSource::Vst3 => "vst3",
            LiveSource::Fluid => "fluid",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "thru" => Some(LiveSource::Thru),
            "vst3" => Some(LiveSource::Vst3),
            "fluid" => Some(LiveSource::Fluid),
            _ => None,
        }
    }
}

/// Verrou global des tests qui lisent `$HOME` (scan des presets/soundfonts)
/// ou l'écrivent (`api_grilles_cycle_de_vie` isole un HOME temporaire) : les
/// scans exécutés en parallèle voyaient le HOME temporaire → flaky.
#[cfg(test)]
pub static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// État agrégé du moteur live : source courante + sous-états.
pub struct LiveInstrumentState {
    /// Source active (le routage `monitor_send` la consulte à chaque message).
    pub source: Mutex<LiveSource>,
    /// Sous-moteur VST3 (Surge XT).
    pub vst3: Arc<Vst3LiveState>,
    /// Instrument GM posé sur FluidSynth (None = pas encore choisi).
    pub fluid_program: Mutex<Option<u8>>,
    /// SoundFont choisie pour FluidSynth (None = celle du serveur).
    pub fluid_soundfont: Mutex<Option<String>>,
}

impl LiveInstrumentState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            source: Mutex::new(LiveSource::Thru),
            vst3: Vst3LiveState::new(),
            fluid_program: Mutex::new(None),
            fluid_soundfont: Mutex::new(None),
        })
    }
}

/// État courant du moteur live (GET /live-instrument).
/// `vst3` : {enabled, preset, error} ; `fluid` : {program, soundfont}.
pub fn status_live(state: &Arc<LiveInstrumentState>) -> serde_json::Value {
    let source = state.source.lock().unwrap();
    let vst3 = status(&state.vst3);
    let fluid_program = state.fluid_program.lock().unwrap().clone();
    let fluid_soundfont = state.fluid_soundfont.lock().unwrap().clone();
    serde_json::json!({
        "source": source.as_str(),
        "vst3": vst3,
        "fluid": {
            "program": fluid_program,
            "soundfont": fluid_soundfont,
        },
    })
}

// ─── Presets Surge XT ───────────────────────────────────────────────────

/// Presets « best-of » curatés par catégorie (noms vérifiés dans
/// patches_factory d'usine — voir memory 2026-08-21).
pub const BEST_PRESETS: &[(&str, &str)] = &[
    ("Keys", "DX EP"), ("Keys", "Digi Harpsi"), ("Keys", "Church"),
    ("Keys", "Dirt"), ("Keys", "Artificial 1"), ("Keys", "Circus 1"),
    ("Keys", "Soft Suitcase"), ("Keys", "Artificial 2"),
    ("Leads", "Acidofil"), ("Leads", "Agroculture"), ("Leads", "Asymptote"),
    ("Leads", "Banjo Remains"), ("Leads", "Bee"), ("Leads", "Bitten"),
    ("Leads", "Banter"), ("Leads", "Bad Childhood"),
    ("Pads", "Alias Pornography"), ("Pads", "Bell Pad"), ("Pads", "Bells and Sweep"),
    ("Pads", "Choir Pad Thing"), ("Pads", "Bright"), ("Pads", "Burden"),
    ("Pads", "Canadians"), ("Pads", "Assymetry"),
    ("Basses", "Behemoth"), ("Basses", "Crush Bass"), ("Basses", "Attacky"),
    ("Basses", "Bass 2"), ("Basses", "Bass 4"), ("Basses", "Bass 5"),
    ("Plucks", "80s Gliss"), ("Plucks", "ACME"), ("Plucks", "Agropop"),
    ("Plucks", "Ambient E-Guitar"), ("Plucks", "Bell 1"), ("Plucks", "Battered Beauty"),
    ("Plucks", "Artificial"), ("Plucks", "Asymmetry"),
    ("Polysynths", "1804"), ("Polysynths", "Ahh Polly"), ("Polysynths", "Analyse"),
    ("Polysynths", "Anthemish 1"), ("Polysynths", "Anthemish 2"), ("Polysynths", "Anthemish 3"),
    ("Polysynths", "Boss"), ("Polysynths", "Bolibompa"),
    ("Brass", "Brassy"), ("Brass", "JX-10 Double Brass"), ("Brass", "OB-8 Jump"),
    ("Brass", "Synth Brass 1"), ("Brass", "Plastic Brass"), ("Brass", "Reso Brassy"),
    ("Winds", "Clarinet"), ("Winds", "Cyber Flute"), ("Winds", "Dreamy Flute"),
    ("Winds", "Flute 1"), ("Winds", "Flute 2"), ("Winds", "Tragic Winds"),
    ("Winds", "Low"),
    ("Percussion", "Drum One"), ("Percussion", "Kick 909ish"),
    ("Percussion", "Snare Tight"), ("Percussion", "Synth Tom 1"),
    ("Sequences", "Acid Seq 1"), ("Sequences", "Acid Seq 2"),
    ("Sequences", "Bell Seq"), ("Sequences", "Bit Seq"),
    ("Chords", "Major 7 MkI"), ("Chords", "Major 7 MkII"),
    ("Chords", "Minor 7"), ("Chords", "Inharmonic Stab"),
    ("MPE", "Bloom"), ("MPE", "Lead With A Bell Attack"),
    ("MPE", "Moving Day In The Caves"), ("MPE", "Pad Plink 'n' Move"),
];

/// Vrai si (catégorie, nom) fait partie du best-of.
pub fn is_best_preset(category: &str, name: &str) -> bool {
    BEST_PRESETS.iter().any(|(c, n)| *c == category && *n == name)
}

/// État partagé du moteur VST3 live (routes HTTP + callback d'entrée MIDI
/// + callback audio). Tout ce qui est non-`Send` (stream cpal, hôte VST3)
/// vit sur le thread moteur — jamais dans cet état.
pub struct Vst3LiveState {
    /// Moteur actif (le monitoring passe par le plugin au lieu du thru MIDI).
    pub enabled: AtomicBool,
    /// Chemin absolu du preset .fxp courant.
    pub preset_path: Mutex<Option<String>>,
    /// Nom lisible du preset courant.
    pub preset_name: Mutex<Option<String>>,
    /// Dernière erreur (affichée par l'API — pas de panique silencieuse).
    pub last_error: Mutex<Option<String>>,
    /// Le stream audio cpal a signalé une erreur (ex: POLLERR — device
    /// ALSA mort) : le thread moteur le détecte et le redémarre (backoff).
    pub stream_error: AtomicBool,
    /// File MIDI : alimentée par le callback d'entrée (live_input), vidée par
    /// le callback audio au début de chaque block (latence ≤ 1 buffer).
    queue: Arc<Mutex<VecDeque<MidiEvent>>>,
    /// Plugin chargé (None si arrêté) — partagé entre le thread moteur
    /// (lock normal) et le callback audio (try_lock).
    plugin: Arc<Mutex<Option<Plugin>>>,
    /// Canal de commandes vers le thread moteur (créé au premier start).
    cmd_tx: Mutex<Option<Sender<Cmd>>>,
    /// Handle du thread moteur (gardé vivant ; jamais join — le process se
    /// termine avec main).
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Vst3LiveState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(false),
            preset_path: Mutex::new(None),
            preset_name: Mutex::new(None),
            last_error: Mutex::new(None),
            stream_error: AtomicBool::new(false),
            queue: Arc::new(Mutex::new(VecDeque::new())),
            plugin: Arc::new(Mutex::new(None)),
            cmd_tx: Mutex::new(None),
            thread: Mutex::new(None),
        })
    }
}

/// Un preset Surge : nom affichable, chemin absolu, catégorie (dossier 1er
/// niveau de patches_factory : Leads, Pads, Basses…), best-of ⭐.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PresetInfo {
    pub name: String,
    pub path: String,
    pub category: String,
    /// Vrai si le preset fait partie du best-of curaté.
    pub best: bool,
}

/// Une SoundFont trouvée sur le système (.sf2/.sf3).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SoundfontInfo {
    pub name: String,
    pub path: String,
    pub kind: String, // "sf2" | "sf3"
    pub size: u64,
}

/// Racine des presets d'usine Surge XT — chemins standards par plateforme :
/// Linux `~/.local/share/Surge XT/patches_factory`, macOS
/// `~/Library/Application Support/Surge XT/patches_factory`, Windows
/// `%APPDATA%/Surge XT/patches_factory`. Variable d'env SURGE_PATCHES_DIR
/// prioritaire (le script d'installation des standalone peut la poser).
pub fn patches_root() -> std::path::PathBuf {
    if let Ok(d) = std::env::var("SURGE_PATCHES_DIR") {
        return std::path::PathBuf::from(d);
    }
    match std::env::consts::OS {
        "macos" => std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join("Library/Application Support/Surge XT/patches_factory"),
        "windows" => {
            let base = std::env::var("APPDATA").unwrap_or_default();
            std::path::PathBuf::from(base).join("Surge XT/patches_factory")
        }
        _ => std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join(".local/share/Surge XT/patches_factory"),
    }
}

/// Scan récursif des presets `.fxp` — catégorie = dossier parent direct
/// (nom du dossier), tri par catégorie puis nom.
pub fn list_presets() -> Vec<PresetInfo> {
    let mut out = Vec::new();
    let root = patches_root();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return out;
    };
    let mut stack: Vec<(std::path::PathBuf, Option<String>)> =
        entries.flatten().map(|e| (e.path(), None)).collect();
    while let Some((dir, cat)) = stack.pop() {
        if !dir.is_dir() {
            continue;
        }
        // Premier niveau : la catégorie. Niveaux plus profonds : on garde la
        // catégorie d'origine (ex. Tutorials/Formula Modulator → Tutorials).
        let category = cat.clone().unwrap_or_else(|| {
            dir.file_name().unwrap_or_default().to_string_lossy().into_owned()
        });
        if let Ok(sub) = std::fs::read_dir(&dir) {
            for e in sub.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push((p, Some(category.clone())));
                } else if p.extension().map(|x| x.eq_ignore_ascii_case("fxp")).unwrap_or(false) {
                    let name = p
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    out.push(PresetInfo {
                        name: name.clone(),
                        path: p.display().to_string(),
                        category: category.clone(),
                        best: is_best_preset(&category, &name),
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| {
        a.category
            .to_lowercase()
            .cmp(&b.category.to_lowercase())
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

/// Extrait le state XML d'un `.fxp` Surge : le fichier = header binaire +
/// XML (l'état du plugin). `load_preset` refuse ce format non standard ;
/// `load_state` sur le XML fonctionne. Fonction pure (testable).
pub fn extract_xml_state(data: &[u8]) -> Option<&[u8]> {
    let marker = b"<?xml";
    data.windows(marker.len()).position(|w| w == marker).map(|pos| &data[pos..])
}

/// Dossiers VST3 à scanner selon la plateforme (chemins standards) :
/// - Linux : `~/.vst3`
/// - macOS : `~/.vst3` + `~/Library/Audio/Plug-Ins/VST3` + `/Library/Audio/Plug-Ins/VST3`
/// - Windows : `%USERPROFILE%\.vst3` + `%ProgramFiles%\Common Files\VST3` +
///   `%ProgramFiles(x86)%\Common Files\VST3`
/// (les installeurs officiels — Surge XT .dmg/.exe — déposent les plugins
/// dans les dossiers standards de la plateforme, pas dans ~/.vst3).
pub fn vst3_search_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
    if !home.is_empty() {
        dirs.push(std::path::PathBuf::from(&home).join(".vst3"));
    }
    match std::env::consts::OS {
        "macos" => {
            if !home.is_empty() {
                dirs.push(std::path::PathBuf::from(&home).join("Library/Audio/Plug-Ins/VST3"));
            }
            dirs.push(std::path::PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        }
        "windows" => {
            for var in ["ProgramFiles", "ProgramFiles(x86)"] {
                if let Ok(pf) = std::env::var(var) {
                    dirs.push(std::path::PathBuf::from(pf).join("Common Files/VST3"));
                }
            }
        }
        _ => {}
    }
    dirs
}

/// Cherche le chemin du plugin Surge XT : `~/.vst3/Surge XT.vst3` d'abord,
/// sinon premier .vst3 (dans tous les dossiers standards de la plateforme)
/// dont le nom contient « surge ».
pub fn find_surge_plugin() -> Result<String, String> {
    for dir in vst3_search_dirs() {
        let direct = dir.join("Surge XT.vst3");
        if direct.exists() {
            return Ok(direct.display().to_string());
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            if p.extension().map(|x| x.eq_ignore_ascii_case("vst3")).unwrap_or(false)
                && name.contains("surge")
            {
                return Ok(p.display().to_string());
            }
        }
    }
    Err("Plugin Surge XT introuvable dans les dossiers VST3 standards (installé ?)".into())
}

/// Résout un preset : chemin direct existant, sinon recherche dans
/// patches_factory — d'abord par NOM DE FICHIER exact (le frontend peut
/// envoyer un chemin relatif), puis par nom partiel. Retourne (chemin, nom).
pub fn resolve_preset(arg: &str) -> Result<(String, String), String> {
    let path = std::path::Path::new(arg);
    if path.exists() {
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        return Ok((path.display().to_string(), name));
    }
    // Nom de fichier exact (même derrière un chemin relatif) :
    // « …/MPE/Pad Plink 'n' Move.fxp » → cherche « Pad Plink 'n' Move ».
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    if !stem.is_empty() {
        for p in list_presets() {
            if p.name == stem {
                return Ok((p.path.clone(), p.name));
            }
        }
    }
    // Recherche par nom partiel (comportement historique)
    let lower = arg.to_lowercase();
    for p in list_presets() {
        if p.name.to_lowercase().contains(&lower) {
            return Ok((p.path.clone(), p.name));
        }
    }
    Err(format!("Preset « {arg} » introuvable dans {}", patches_root().display()))
}

/// Normalise un chemin d'instrument envoyé par le frontend (rendu WAV) :
/// - chemin absolu existant → conservé ;
/// - `~/…` → expandu avec HOME ;
/// - preset Surge `.fxp` relatif → résolu par nom de fichier dans
///   patches_factory (le frontend peut envoyer « .local/share/…/X.fxp ») ;
/// - autre chemin relatif (sfz/sf2/vst3) → préfixé par HOME.
pub fn resolve_instrument_path(kind: &str, path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let p = path.trim();
    if p.starts_with("~/") {
        return format!("{}/{}", home, &p[2..]);
    }
    let abs = std::path::Path::new(p);
    if abs.is_absolute() {
        return p.to_string();
    }
    if kind == "vst3" && p.to_lowercase().ends_with(".fxp") {
        if let Ok((full, _)) = resolve_preset(p) {
            return full;
        }
    }
    format!("{}/{}", home, p)
}

/// Applique un preset .fxp Surge au plugin (extraction XML + load_state).
fn apply_preset_file(plugin: &mut Plugin, preset_path: &str) -> Result<(), String> {
    let data = std::fs::read(preset_path)
        .map_err(|e| format!("Lecture du preset impossible : {e}"))?;
    let xml = extract_xml_state(&data)
        .ok_or_else(|| format!("Pas de XML trouvé dans {preset_path}"))?;
    plugin
        .load_state(xml)
        .map_err(|e| format!("load_state : {e}"))
}

/// Convertit un message MIDI brut en événement VST3 (notes, CC, PC, bend).
/// Fonction pure (testable sans matériel).
pub fn midi_to_vst3_event(msg: &[u8]) -> Option<MidiEvent> {
    if msg.len() < 2 {
        return None;
    }
    let status = msg[0];
    let ch = MidiChannel::from_index(status & 0x0F).unwrap_or(MidiChannel::Ch1);
    match status & 0xF0 {
        0x90 => {
            let vel = msg.get(2).copied().unwrap_or(0);
            if vel > 0 {
                Some(MidiEvent::NoteOn { channel: ch, note: msg[1], velocity: vel })
            } else {
                Some(MidiEvent::NoteOff { channel: ch, note: msg[1], velocity: 0 })
            }
        }
        0x80 => Some(MidiEvent::NoteOff {
            channel: ch,
            note: msg[1],
            velocity: msg.get(2).copied().unwrap_or(0),
        }),
        0xB0 => Some(MidiEvent::ControlChange {
            channel: ch,
            controller: msg[1],
            value: msg.get(2).copied().unwrap_or(0),
        }),
        0xC0 => Some(MidiEvent::ProgramChange { channel: ch, program: msg[1] }),
        0xE0 => {
            let lsb = msg.get(1).copied().unwrap_or(0) as u16;
            let msb = msg.get(2).copied().unwrap_or(0) as u16;
            Some(MidiEvent::PitchBend { channel: ch, value: lsb | (msb << 7) })
        }
        _ => None,
    }
}

/// Pousse un message MIDI brut dans la file du moteur. Sans effet si le
/// moteur est arrêté. Appelé depuis le callback d'entrée MIDI (live_input).
pub fn enqueue(state: &Arc<Vst3LiveState>, msg: &[u8]) {
    if !state.enabled.load(Ordering::Relaxed) {
        return;
    }
    if let Some(ev) = midi_to_vst3_event(msg) {
        if let Ok(mut q) = state.queue.lock() {
            q.push_back(ev);
        }
    }
}

/// Garantit que le thread moteur existe et retourne son canal de commandes.
fn engine_channel(state: &Arc<Vst3LiveState>) -> Result<Sender<Cmd>, String> {
    let mut guard = state.cmd_tx.lock().unwrap();
    if guard.is_none() {
        let (tx, rx) = mpsc::channel();
        let st = Arc::clone(state);
        let handle = std::thread::Builder::new()
            .name("vst3-live".into())
            .spawn(move || engine_loop(st, rx))
            .map_err(|e| format!("Impossible de lancer le thread moteur : {e}"))?;
        *state.thread.lock().unwrap() = Some(handle);
        *guard = Some(tx);
    }
    Ok(guard.clone().unwrap())
}

/// Démarre le moteur : plugin + preset + stream audio vers le Roland.
/// Si le moteur tourne déjà, change seulement le preset (à chaud).
pub fn start(state: &Arc<Vst3LiveState>, preset_arg: &str) -> Result<(), String> {
    let (preset_path, preset_name) = resolve_preset(preset_arg)?;
    let tx = engine_channel(state)?;
    let (reply, rx) = mpsc::channel();
    tx.send(Cmd::Start { preset: preset_path.clone(), reply })
        .map_err(|_| "Thread moteur arrêté".to_string())?;
    let res = rx.recv().map_err(|_| "Thread moteur arrêté".to_string())?;
    if res.is_ok() {
        *state.preset_path.lock().unwrap() = Some(preset_path);
        *state.preset_name.lock().unwrap() = Some(preset_name);
    }
    res
}

/// Change le preset à chaud (moteur actif) — glitch audio bref possible.
pub fn set_preset(state: &Arc<Vst3LiveState>, preset_arg: &str) -> Result<(), String> {
    let (preset_path, preset_name) = resolve_preset(preset_arg)?;
    let tx = engine_channel(state)?;
    let (reply, rx) = mpsc::channel();
    tx.send(Cmd::SetPreset { preset: preset_path.clone(), reply })
        .map_err(|_| "Thread moteur arrêté".to_string())?;
    let res = rx.recv().map_err(|_| "Thread moteur arrêté".to_string())?;
    if res.is_ok() {
        *state.preset_path.lock().unwrap() = Some(preset_path);
        *state.preset_name.lock().unwrap() = Some(preset_name);
    }
    res
}

/// Arrête le moteur et libère le device audio (drop du stream sur le
/// thread moteur). Sans effet si déjà arrêté.
pub fn stop(state: &Arc<Vst3LiveState>) {
    let Some(tx) = state.cmd_tx.lock().unwrap().clone() else {
        return;
    };
    let (reply, rx) = mpsc::channel();
    if tx.send(Cmd::Stop { reply }).is_ok() {
        let _ = rx.recv();
    }
}

/// État courant pour l'API (GET /live-vst3).
pub fn status(state: &Arc<Vst3LiveState>) -> serde_json::Value {
    let preset_path = state.preset_path.lock().unwrap().clone();
    let preset_name = state.preset_name.lock().unwrap().clone();
    let last_error = state.last_error.lock().unwrap().clone();
    serde_json::json!({
        "enabled": state.enabled.load(Ordering::Relaxed),
        "preset": preset_path.map(|p| serde_json::json!({
            "path": p,
            "name": preset_name.unwrap_or_else(|| p.split('/').last().unwrap_or(&p).to_string()),
        })),
        "error": last_error,
    })
}

// ─── Thread moteur (possède le stream cpal, non-Send) ───────────────────

/// Boucle du thread moteur : reçoit les commandes, garde le stream + l'hôte
/// VST3 (non-`Send`) dans `live`. Le plugin est déposé dans l'état partagé
/// (il est `Send` — le callback audio ALSA le verrouille en try_lock).
fn engine_loop(state: Arc<Vst3LiveState>, rx: Receiver<Cmd>) {
    let mut live: Option<(Vst3Host, cpal::Stream)> = None;
    // Backoff du redémarrage automatique du stream (1 s → 2 → 4 … max 30 s).
    let mut restart_delay: u64 = 1;
    loop {
        match rx.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(Cmd::Start { preset, reply }) => {
                let res = engine_start(&state, &mut live, &preset);
                if res.is_ok() {
                    restart_delay = 1;
                }
                let _ = reply.send(res);
            }
            Ok(Cmd::SetPreset { preset, reply }) => {
                let res = engine_set_preset(&state, &preset);
                let _ = reply.send(res);
            }
            Ok(Cmd::Stop { reply }) => {
                engine_stop(&state, &mut live);
                state.stream_error.store(false, Ordering::Relaxed);
                let _ = reply.send(());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // ── Surveillance : le stream audio est mort (POLLERR…) ? ──
                // Le device ALSA du Roland peut être perdu (conflit PipeWire,
                // xrun sévère, réinitialisation USB) : au lieu de rester
                // muet « enabled », on redémarre le moteur avec backoff.
                if state.stream_error.swap(false, Ordering::Relaxed) {
                    let preset = state.preset_path.lock().unwrap().clone();
                    match preset {
                        Some(p) => {
                            eprintln!("🔁 Stream audio VST3 live mort — redémarrage dans {restart_delay}s…");
                            std::thread::sleep(std::time::Duration::from_secs(restart_delay));
                            // Redémarrage complet : drop du stream/plugin, re-start.
                            engine_stop(&state, &mut live);
                            // Vide la file MIDI accumulée pendant la panne.
                            state.queue.lock().unwrap().clear();
                            let res = engine_start(&state, &mut live, &p);
                            match res {
                                Ok(()) => {
                                    restart_delay = 1;
                                    *state.last_error.lock().unwrap() = None;
                                    eprintln!("✅ Stream audio VST3 live rétabli");
                                }
                                Err(e) => {
                                    restart_delay = (restart_delay * 2).min(30);
                                    *state.last_error.lock().unwrap() = Some(e);
                                }
                            }
                        }
                        None => {
                            // Pas de preset connu : le moteur ne sert plus à
                            // rien → arrêt propre (retour au thru MIDI).
                            engine_stop(&state, &mut live);
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Démarre réellement le moteur (appelé sur le thread moteur).
fn engine_start(
    state: &Arc<Vst3LiveState>,
    live: &mut Option<(Vst3Host, cpal::Stream)>,
    preset_path: &str,
) -> Result<(), String> {
    if live.is_some() {
        // Déjà actif → simple changement de preset à chaud.
        return engine_set_preset(state, preset_path);
    }

    // Plugin + preset
    let plugin_path = find_surge_plugin()?;
    let mut host = Vst3Host::builder()
        .sample_rate(LIVE_RATE as f64)
        .block_size(LIVE_BUFFER)
        .build()
        .map_err(|e| format!("Création de l'hôte VST3 impossible : {e}"))?;
    let mut plugin = host
        .load_plugin(&plugin_path)
        .map_err(|e| format!("Chargement du plugin impossible : {e}"))?;
    apply_preset_file(&mut plugin, preset_path)?;
    plugin
        .start_processing()
        .map_err(|e| format!("start_processing : {e}"))?;

    // Sortie audio : device « roland » (plug ~/.asoundrc → hw:3,0), la seule
    // config validée avec le Roland FP-60X (44,1 kHz + buffer 256).
    let host_d = cpal::default_host();
    let device = host_d
        .output_devices()
        .map_err(|e| format!("Énumération audio impossible : {e}"))?
        .find(|d| {
            d.name()
                .map(|n| n.to_lowercase().contains("roland"))
                .unwrap_or(false)
        })
        .ok_or_else(|| {
            "Device audio « roland » introuvable — le Roland FP-60X est branché en USB ? \
             (~/.asoundrc : pcm.roland = plug → hw:3,0)"
                .to_string()
        })?;
    let dev_name = device.name().map_err(|e| e.to_string())?;
    let def_cfg = device.default_output_config().map_err(|e| e.to_string())?;
    let dev_channels = def_cfg.channels() as usize;
    let stream_cfg = cpal::StreamConfig {
        channels: dev_channels as u16,
        sample_rate: cpal::SampleRate(LIVE_RATE),
        buffer_size: cpal::BufferSize::Fixed(LIVE_BUFFER as u32),
    };
    let p_out = plugin.output_channel_count().max(1);

    // Callback audio temps réel : try_lock (jamais de blocage du thread RT),
    // zéro allocation, file vidée au début du block (latence ≤ 1 buffer).
    let plugin_rt = Arc::clone(&state.plugin);
    let q_rt = Arc::clone(&state.queue);
    // Callback d'ERREUR du stream : pose le flag (le thread moteur redémarre
    // le stream avec backoff) + expose l'erreur à l'API. Throttle du log :
    // POLLERR peut pleuvoir (device ALSA mort) — sans limite, le log gonfle
    // à l'infini (9,6 Go observés !).
    let err_state = Arc::clone(&state);
    let mut last_err_log = std::time::Instant::now();
    let err_fn = move |e: cpal::StreamError| {
        err_state.stream_error.store(true, Ordering::Relaxed);
        *err_state.last_error.lock().unwrap() =
            Some(format!("Audio VST3 live : {e}"));
        if last_err_log.elapsed().as_secs() >= 1 {
            eprintln!("⚠️ Erreur audio VST3 live : {e}");
            last_err_log = std::time::Instant::now();
        }
    };
    let stream = device
        .build_output_stream(
            &stream_cfg,
            move |data: &mut [f32], _| {
                let mut guard = match plugin_rt.try_lock() {
                    Ok(g) => g,
                    Err(_) => {
                        data.fill(0.0);
                        return;
                    }
                };
                let Some(pl) = guard.as_mut() else {
                    data.fill(0.0);
                    return;
                };
                if let Ok(mut q) = q_rt.try_lock() {
                    while let Some(ev) = q.pop_front() {
                        let _ = pl.send_midi_event_at(ev, 0);
                    }
                }
                let frames = data.len() / dev_channels.max(1);
                if frames == 0 {
                    return;
                }
                let mut buffers =
                    AudioBuffers::new(0, p_out, frames, LIVE_RATE as f64);
                if pl.process_audio(&mut buffers).is_err() {
                    data.fill(0.0);
                    return;
                }
                let l = buffers.outputs.first().map(|v| v.as_slice()).unwrap_or(&[]);
                let r = buffers.outputs.get(1).map(|v| v.as_slice()).unwrap_or(l);
                // Gain de sécurité (≈ −2 dB) : les presets Surge chauds ne
                // saturent pas l'entrée audio USB du Roland. Clamp final.
                const LIVE_GAIN: f32 = 0.8;
                if dev_channels >= 2 {
                    for i in 0..frames {
                        data[i * 2] = (l.get(i).copied().unwrap_or(0.0) * LIVE_GAIN).clamp(-0.98, 0.98);
                        data[i * 2 + 1] = (r.get(i).copied().unwrap_or(0.0) * LIVE_GAIN).clamp(-0.98, 0.98);
                    }
                } else {
                    for i in 0..frames {
                        let m = (l.get(i).copied().unwrap_or(0.0) + r.get(i).copied().unwrap_or(0.0)) * 0.5;
                        data[i] = (m * LIVE_GAIN).clamp(-0.98, 0.98);
                    }
                }
            },
            err_fn,
            None,
        )
        .map_err(|e| format!("Ouverture du device audio impossible : {e}"))?;
    stream
        .play()
        .map_err(|e| format!("stream.play : {e}"))?;

    // Rendre l'état visible (ordre : le plugin d'abord, enabled ensuite —
    // le callback enqueue ne reçoit rien tant que enabled est false).
    *state.plugin.lock().unwrap() = Some(plugin);
    *live = Some((host, stream));
    state.enabled.store(true, Ordering::Relaxed);
    println!("🎛️  Moteur VST3 live ACTIF → {dev_name} ({dev_channels} canaux)");
    Ok(())
}

/// Change le preset à chaud (appelé sur le thread moteur).
fn engine_set_preset(state: &Arc<Vst3LiveState>, preset_path: &str) -> Result<(), String> {
    let mut guard = state.plugin.lock().unwrap();
    match guard.as_mut() {
        Some(pl) => apply_preset_file(pl, preset_path),
        None => Err("Moteur VST3 live non actif".to_string()),
    }
}

/// Arrête le moteur (appelé sur le thread moteur) : drop du stream et de
/// l'hôte (libère le device audio), plugin retiré de l'état partagé.
fn engine_stop(state: &Arc<Vst3LiveState>, live: &mut Option<(Vst3Host, cpal::Stream)>) {
    *live = None;
    *state.plugin.lock().unwrap() = None;
    state.enabled.store(false, Ordering::Relaxed);
    println!("🛑 Moteur VST3 live arrêté (thru MIDI de nouveau actif)");
}

/// Scan des SoundFonts .sf2/.sf3 sur le système : dossier système
/// (/usr/share/sounds) + ~/soundfonts (banques ajoutées à la main).
/// Retourne la SoundFont du serveur (MuseScore General Full) en premier.
pub fn scan_soundfonts() -> Vec<SoundfontInfo> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut roots: Vec<std::path::PathBuf> = vec![
        std::path::PathBuf::from("/usr/share/sounds"),
        std::path::PathBuf::from(&home).join("soundfonts"),
        std::path::PathBuf::from(&home).join(".local/share/soundfonts"),
    ];
    let mut out = Vec::new();
    for root in roots.drain(..) {
        if !root.is_dir() {
            continue;
        }
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if let Some(ext) = p.extension() {
                        let kind = ext.to_string_lossy().to_lowercase();
                        if kind == "sf2" || kind == "sf3" {
                            if let Ok(meta) = std::fs::metadata(&p) {
                                let name = p
                                    .file_stem()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned();
                                out.push(SoundfontInfo {
                                    name,
                                    path: p.display().to_string(),
                                    kind,
                                    size: meta.len(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    // La SoundFont du serveur en premier (banque par défaut du moteur live
    // FluidSynth) — les doublons de nom sont gardés (chemins différents).
    if let Some(sf) = find_system_soundfont() {
        if let Some(pos) = out.iter().position(|s| s.path == sf) {
            let item = out.remove(pos);
            out.insert(0, item);
        }
    }
    out
}

/// Cherche la SoundFont principale du serveur (MuseScore General Full).
fn find_system_soundfont() -> Option<String> {
    for cand in [
        "/usr/share/sounds/sf3/MuseScore_General_Full.sf3",
        "/usr/share/sounds/sf3/MuseScore_General.sf3",
        "/usr/share/sounds/sf2/FluidR3_GM.sf2",
    ] {
        if std::path::Path::new(cand).exists() {
            return Some(cand.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_xml_depuis_fxp_surge() {
        // Header binaire factice + XML (comme un .fxp Surge)
        let mut data = vec![0u8; 64];
        data[0..4].copy_from_slice(b"fxpH");
        data.extend_from_slice(b"<?xml version=\"1.0\"?><surge>state</surge>");
        let xml = extract_xml_state(&data).unwrap();
        assert_eq!(xml, b"<?xml version=\"1.0\"?><surge>state</surge>");
        // Sans marqueur XML → None
        assert_eq!(extract_xml_state(b"pas de xml ici"), None);
    }

    #[test]
    fn conversion_midi_vers_vst3() {
        // Note-on vel > 0
        assert_eq!(
            midi_to_vst3_event(&[0x90, 60, 100]),
            Some(MidiEvent::NoteOn { channel: MidiChannel::Ch1, note: 60, velocity: 100 })
        );
        // Note-on vel 0 = note-off
        assert_eq!(
            midi_to_vst3_event(&[0x90, 60, 0]),
            Some(MidiEvent::NoteOff { channel: MidiChannel::Ch1, note: 60, velocity: 0 })
        );
        // Note-off classique
        assert_eq!(
            midi_to_vst3_event(&[0x80, 64, 64]),
            Some(MidiEvent::NoteOff { channel: MidiChannel::Ch1, note: 64, velocity: 64 })
        );
        // CC (canal 5)
        assert_eq!(
            midi_to_vst3_event(&[0xB5, 64, 127]),
            Some(MidiEvent::ControlChange { channel: MidiChannel::Ch6, controller: 64, value: 127 })
        );
        // Program change
        assert_eq!(
            midi_to_vst3_event(&[0xC2, 51]),
            Some(MidiEvent::ProgramChange { channel: MidiChannel::Ch3, program: 51 })
        );
        // Pitch bend : lsb | msb<<7
        assert_eq!(
            midi_to_vst3_event(&[0xE0, 0x00, 0x40]),
            Some(MidiEvent::PitchBend { channel: MidiChannel::Ch1, value: 8192 })
        );
        // Messages ignorés
        assert_eq!(midi_to_vst3_event(&[0xF0]), None); // SysEx (trop court)
        assert_eq!(midi_to_vst3_event(&[0x90]), None); // trop court
        assert_eq!(midi_to_vst3_event(&[]), None);
    }

    #[test]
    fn scan_presets_surge() {
        let _guard = HOME_LOCK.lock().unwrap(); // $HOME est partagé entre tests
        // Test d'intégration : la machine de dev a Surge XT installé.
        let presets = list_presets();
        assert!(presets.len() >= 600, "attendu ≥ 600 presets, trouvé {}", presets.len());
        // Catégories attendues (patches_factory d'usine)
        let cats: std::collections::HashSet<&str> =
            presets.iter().map(|p| p.category.as_str()).collect();
        for want in ["Leads", "Pads", "Basses", "Plucks", "Polysynths"] {
            assert!(cats.contains(want), "catégorie {want} manquante");
        }
        // Chaque preset a un chemin absolu .fxp
        for p in presets.iter().take(10) {
            assert!(p.path.ends_with(".fxp"), "{}", p.path);
        }
        // Recherche par nom partiel
        let resolved = resolve_preset("Soft Suitcase").unwrap();
        assert!(resolved.0.ends_with(".fxp"));
        // Chemin direct
        let direct = resolve_preset(&resolved.0).unwrap();
        assert_eq!(direct.0, resolved.0);
        // Introuvable → erreur
        assert!(resolve_preset("zzz_preset_inexistant_123").is_err());
    }

    #[test]
    fn best_of_tous_les_presets_existent() {
        let _guard = HOME_LOCK.lock().unwrap(); // $HOME est partagé entre tests
        // Chaque entrée du best-of doit exister dans patches_factory
        // (catégorie + nom) — sinon le ⭐ ne mènerait nulle part.
        let presets = list_presets();
        let by_cat: std::collections::HashMap<&str, std::collections::HashSet<&str>> = presets
            .iter()
            .map(|p| (p.category.as_str(), p.name.as_str()))
            .fold(std::collections::HashMap::new(), |mut m, (c, n)| {
                m.entry(c).or_default().insert(n);
                m
            });
        let mut missing = Vec::new();
        for (cat, name) in BEST_PRESETS {
            if !by_cat.get(cat).map(|s| s.contains(name)).unwrap_or(false) {
                missing.push(format!("{cat}/{name}"));
            }
        }
        assert!(missing.is_empty(), "best-of introuvables : {missing:?}");
        // Tous marqués best dans le scan
        let best_count = presets.iter().filter(|p| p.best).count();
        assert_eq!(best_count, BEST_PRESETS.len());
    }

    #[test]
    fn scan_soundfonts_trouve_des_banques() {
        let _guard = HOME_LOCK.lock().unwrap(); // $HOME est partagé entre tests
        let sfs = scan_soundfonts();
        assert!(sfs.len() >= 3, "attendu ≥ 3 soundfonts, trouvé {}", sfs.len());
        // La SoundFont du serveur (MuseScore General Full) est en premier
        assert!(sfs[0].path.contains("MuseScore_General_Full"));
        for s in sfs.iter().take(5) {
            assert!(s.kind == "sf2" || s.kind == "sf3");
            assert!(s.size > 0);
        }
    }

    #[test]
    fn sources_parse_et_affichage() {
        assert_eq!(LiveSource::from_str("thru"), Some(LiveSource::Thru));
        assert_eq!(LiveSource::from_str("vst3"), Some(LiveSource::Vst3));
        assert_eq!(LiveSource::from_str("fluid"), Some(LiveSource::Fluid));
        assert_eq!(LiveSource::from_str("autre"), None);
        assert_eq!(LiveSource::Thru.as_str(), "thru");
        assert_eq!(LiveSource::Fluid.as_str(), "fluid");
    }

    #[test]
    fn resolution_chemins_instruments() {
        let _guard = HOME_LOCK.lock().unwrap(); // $HOME est partagé entre tests
        let home = std::env::var("HOME").unwrap_or_default();
        // Chemin absolu conservé
        let abs = format!("{home}/.local/share/Surge XT/patches_factory/Keys/DX EP.fxp");
        assert_eq!(resolve_instrument_path("vst3", &abs), abs);
        // Tilde expandu
        assert_eq!(
            resolve_instrument_path("sfz", "~/Dev/banque/Test.sfz"),
            format!("{home}/Dev/banque/Test.sfz")
        );
        // Preset .fxp relatif → résolu par nom de fichier dans patches_factory
        // (le cas du bug : « .local/share/Surge XT/patches_factory/MPE/…fxp »)
        let rel = ".local/share/Surge XT/patches_factory/MPE/Pad Plink 'n' Move.fxp";
        let resolved = resolve_instrument_path("vst3", rel);
        assert!(resolved.ends_with("MPE/Pad Plink 'n' Move.fxp"), "{resolved}");
        assert!(resolved.starts_with("/"), "doit être absolu : {resolved}");
        assert!(std::path::Path::new(&resolved).exists(), "{resolved}");
        // Preset .fxp par nom seul
        let by_name = resolve_instrument_path("vst3", "Pad Plink 'n' Move.fxp");
        assert_eq!(by_name, resolved);
        // Chemin relatif simple (sfz) → préfixé par HOME
        assert_eq!(
            resolve_instrument_path("sfz", "Dev/banque/Test.sfz"),
            format!("{home}/Dev/banque/Test.sfz")
        );
    }
}
