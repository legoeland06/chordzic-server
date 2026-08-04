/// Serveur HTTP chordZIC — Backend Axum pour l'application de
/// séquencement d'accords et rendu audio.
///
/// Ce serveur fait le lien entre le frontend React/Vite et le moteur
/// MIDI/audio en arrière-plan.  Il expose 6 routes REST :
///
/// - `GET  /`           : page d'accueil (index.html statique)
/// - `POST /play`       : lance la séquence d'accords en live (MIDI temps réel)
/// - `POST /config`     : modifie la configuration en temps réel (volume, pattern, etc.)
/// - `POST /stop`       : arrête la lecture live
/// - `POST /render-wav` : rendu batch d'une séquence en fichier WAV (via FluidSynth)
/// - `GET  /samples-list` : liste des boucles WAV disponibles
/// - `POST /save`         : sauvegarde une grille en JSON (dossier ~/ChordZIC/grilles/)
/// - `GET  /grilles`      : liste les grilles sauvegardées
/// - `DELETE /grilles/<n>`: supprime une grille
///
/// L'état global `AppState` est partagé entre toutes les routes via Axum
/// State.  La connexion MIDI et l'état live sont encapsulés dans des
/// `Arc` pour le partage entre threads.
///
/// Voir aussi les modules :
/// - `midi`     : communication MIDI live + génération de notes
/// - `patterns` : constantes et identifiants des patterns drums
/// - `render`   : génération SMF + rendu WAV batch
/// - `samples`  : gestion des boucles WAV drums (rodio)
/// - `walking`  : génération de walking bass
/// - `grilles`  : sauvegarde/chargement des grilles en JSON
mod grilles;
mod midi;
mod patterns;
mod render;
mod samples;
mod walking;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
#[cfg(not(feature = "standalone"))]
use axum::response::Html;
use midir::MidiOutput;
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tower_http::cors::CorsLayer;
use patterns::pat;
use midi::{
    apply_tracks, init_midi, note_midi, play_seq, play_notes, rch, pb, ChordEv, Live, LiveTrack,
    MidiHandle, TrackCfg as MidiTrackCfg,
};

// ─── Frontend embarqué (mode standalone) ────────────────────────────────
// Quand on compile avec --features standalone, le frontend React/Vite est
// compilé et embarqué directement dans le binaire Rust via rust-embed.
// Ainsi, un seul fichier exécutable suffit : pas de serveur Vite séparé.
#[cfg(feature = "standalone")]
mod frontend_embed {
    use axum::{
        body::Body,
        extract::Request,
        http::{header, StatusCode},
        response::Response,
    };
    use rust_embed::RustEmbed;

    #[derive(RustEmbed)]
    #[folder = "frontend_dist/"]
    struct FrontendAssets;

    /// Sert les fichiers du frontend embarqué.
    ///
    /// Gère :
    /// - Fichiers statiques (JS, CSS, images) → servis avec leur vrai Content-Type
    /// - SPA fallback : toute route inconnue → index.html (pour le routing React)
    pub async fn serve(req: Request<Body>) -> Response<Body> {
        let path = req.uri().path().trim_start_matches('/');

        // Si le fichier existe dans l'embed, le servir directement
        if let Some(content) = FrontendAssets::get(path) {
            return serve_embedded(path, content);
        }

        // SPA fallback : servir index.html pour les routes React
        // (ne pas capturer les routes API)
        if !path.starts_with("api/") {
            if let Some(content) = FrontendAssets::get("index.html") {
                return serve_embedded("index.html", content);
            }
        }

        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap()
    }

    fn serve_embedded(path: &str, file: rust_embed::EmbeddedFile) -> Response<Body> {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        let mut builder = Response::builder().header(header::CONTENT_TYPE, mime.as_ref());
        // Politique de cache :
        // - index.html : toujours revalider (son contenu change à chaque release)
        // - assets/ (hashés : index-<hash>.js/css) : cache long + immutable
        // - autres : pas de cache
        if path == "index.html" {
            builder = builder.header(header::CACHE_CONTROL, "no-cache");
        } else if path.starts_with("assets/") {
            builder = builder.header(header::CACHE_CONTROL, "public, max-age=31536000, immutable");
        } else {
            builder = builder.header(header::CACHE_CONTROL, "no-cache");
        }
        builder.body(Body::from(file.data)).unwrap()
    }
}

// ─── SoundFont auto-détection ──────────────────────────────────────────

/// Chemins possibles pour la SoundFont MuseScore General Full.
/// Cherche d'abord sur Linux (chemin Debian/Ubuntu), puis macOS (Homebrew
/// Intel et Apple Silicon), puis Homebrew standard, puis les alternatives.
static SF_CANDIDATES: &[&str] = &[
    // Linux (Debian/Ubuntu)
    "/usr/share/sounds/sf3/MuseScore_General_Full.sf3",
    "/usr/share/sounds/sf2/MuseScore_General_Full.sf2",
    // macOS Intel Homebrew
    "/usr/local/share/sounds/sf3/MuseScore_General_Full.sf3",
    "/usr/local/share/sounds/sf2/MuseScore_General_Full.sf2",
    // macOS Apple Silicon Homebrew
    "/opt/homebrew/share/sounds/sf3/MuseScore_General_Full.sf3",
    "/opt/homebrew/share/sounds/sf2/MuseScore_General_Full.sf2",
    // macOS Homebrew (nouveau prefix)
    "/opt/homebrew/opt/fluid-synth/share/sounds/sf3/MuseScore_General_Full.sf3",
    // Fallback : répertoire local dans le même dossier que l'exécutable
    "./MuseScore_General_Full.sf3",
    "./soundfonts/MuseScore_General_Full.sf3",
    // macOS : dans le HOME de l'utilisateur
    "~/MuseScore_General_Full.sf3",
    "~/soundfonts/MuseScore_General_Full.sf3",
    // macOS : Library Audio/Sounds
    "~/Library/Audio/Sounds/MuseScore_General_Full.sf3",
];

/// Trouve le chemin de la SoundFont MuseScore General Full.
///
/// Parcours les candidats dans l'ordre, retourne le premier trouvé.
/// Si rien n'est trouvé, retourne None (le render WAV échouera,
/// mais le live MIDI fonctionnera si FluidSynth tourne déjà).
fn find_soundfont() -> Option<String> {
    for &candidate in SF_CANDIDATES {
        // Expand ~ si présent
        let path = if candidate.starts_with("~/") {
            let home = std::env::var("HOME").unwrap_or_default();
            if home.is_empty() {
                candidate.to_string()
            } else {
                candidate.replacen("~", &home, 1)
            }
        } else {
            candidate.to_string()
        };

        if std::path::Path::new(&path).exists() {
            println!("   🎹 SoundFont trouvée : {}", path);
            return Some(path);
        }
    }

    // Essayer de trouver avec `brew --prefix fluidsynth` sur macOS
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("brew")
            .args(["--prefix", "fluid-synth"])
            .output()
        {
            if output.status.success() {
                let prefix = String::from_utf8_lossy(&output.stdout).trim().to_string();
                for ext in &["sf3", "sf2"] {
                    let path = format!("{}/share/sounds/{}/MuseScore_General_Full.{}", prefix, ext, ext);
                    if std::path::Path::new(&path).exists() {
                        println!("   🎹 SoundFont trouvée via brew : {}", path);
                        return Some(path);
                    }
                }
                // Chercher dans le dossier sounds du prefix
                let sf_dir = format!("{}/share/sounds", prefix);
                if let Ok(entries) = std::fs::read_dir(&sf_dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                            if name.contains("MuseScore") || name.contains("General") {
                                if p.extension().map_or(false, |e| e == "sf3" || e == "sf2") {
                                    println!("   🎹 SoundFont trouvée : {}", p.display());
                                    return Some(p.to_string_lossy().to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    eprintln!("   ⚠️  SoundFont non trouvée. Le rendu WAV ne fonctionnera pas.");
    eprintln!("      Télécharge-la sur : https://musescore.org/fr/telechargement");
    None
}

// ─── État global ────────────────────────────────────────────────────────

/// Connexion MIDI vivante : handle midir + nom du port connecté.
/// Le nom contient le PID du client ALSA (ex: "FLUID Synth (1452036):…")
/// → il change à chaque redémarrage de FluidSynth, ce qui permet de
/// détecter une connexion devenue muette.
struct MidiLink {
    handle: MidiHandle,
    port: String,
}

impl Clone for MidiLink {
    fn clone(&self) -> Self {
        MidiLink {
            handle: Arc::clone(&self.handle),
            port: self.port.clone(),
        }
    }
}

/// État partagé du serveur, injecté dans chaque route via Axum State.
#[derive(Clone)]
struct AppState {
    /// Connexion MIDI vers FluidSynth (None si pas de port).
    /// Arc<Mutex<…>> pour permettre la RECONNEXION automatique depuis les
    /// handlers (la connexion meurt si FluidSynth redémarre).
    midi: Arc<Mutex<Option<MidiLink>>>,
    live: Arc<Live>,            // État live mutable partagé entre threads HTTP et audio
    soundfont: Option<String>,  // Chemin vers la SoundFont (pour render-wav)
}

/// Envoie un message MIDI vers la sortie live, avec reconnexion automatique.
///
/// midir ne signale PAS d'erreur quand le port ALSA a disparu (FluidSynth
/// redémarré) : le message partirait dans le vide. On compare donc le nom du
/// port connecté à la liste des ports actuellement disponibles — s'il n'y est
/// plus, on rouvre une connexion (init_midi) avant d'envoyer.
/// Retourne true si le message est parti.
fn midi_send(state: &AppState, msg: &[u8]) -> bool {
    let mut guard = state.midi.lock().unwrap();

    // Ports actuellement disponibles (énumération légère, ~µs)
    let available: Vec<String> = MidiOutput::new("chords-server-rs")
        .map(|mo| mo.ports().iter().filter_map(|x| mo.port_name(x).ok()).collect())
        .unwrap_or_default();

    // Connexion absente ou port disparu → (re)connecter
    let stale = match guard.as_ref() {
        None => true,
        Some(link) => !available.iter().any(|n| n == &link.port),
    };
    if stale {
        eprintln!("⚠️ Sortie MIDI absente (FluidSynth redémarré ?) — reconnexion automatique…");
        *guard = init_midi().map(|(handle, port)| MidiLink { handle, port });
    }

    let Some(link) = guard.as_ref() else { return false; };
    let Ok(mut conn) = link.handle.lock() else { return false; };
    conn.send(msg).is_ok()
}

// ─── Signature ──────────────────────────────────────────────────────────

/// Encode une signature rythmique textuelle (ex: "4/4") en une valeur
/// numérique compacte : `top * 10 + bottom` → 44 pour 4/4.
///
/// Utilisée comme clé atomique dans `Live.sig` pour éviter de parser
/// une string à chaque beat dans le thread audio.
///
/// # Exemples
/// - "4/4" → 44
/// - "3/4" → 34
/// - "6/8" → 68
/// - "5/4" → 54
fn sig_code(s: &str) -> u16 {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() != 2 {
        return 44; // Défaut 4/4 si format invalide
    }
    let top: u16 = parts[0].parse().unwrap_or(4);
    let bot: u16 = parts[1].parse().unwrap_or(4);
    top * 10 + bot
}

// ─── Requêtes ──────────────────────────────────────────────────────────

/// Requête POST /play — lance la séquence d'accords.
///
/// Tous les champs ont des valeurs par défaut via serde, ce qui permet
/// d'envoyer des requêtes minimales (ex: juste la séquence).

/// Note personnalisée issue du PianoRoll frontend.
/// Utilisée pour remplacer les notes automatiques lors du rendu WAV.
#[derive(Clone, Debug, Deserialize)]
struct CustomNote {
    channel: u8,        // Canal MIDI (0=Lead, 2=Bass, 3=Nappes, 4=Accent, 9=Drums)
    start_time: f64,    // Position en beats
    pitch: u8,          // Note MIDI (0-127)
    duration: f64,      // Durée en beats
    velocity: u8,       // Vélocité (0-127)
}

#[derive(Deserialize)]
struct PlayReq {
    notes: Option<Vec<String>>,     // Notes à jouer immédiatement (pas de séquence)
    #[serde(default)]
    seq: Vec<ChordEv>,               // Séquence d'accords (nom préféré)
    #[serde(default)]
    sequence: Vec<ChordEv>,          // Séquence d'accords (alias, pour compatibilité)
    #[serde(default = "t120")]
    tempo: u32,                      // BPM (120 par défaut)
    #[serde(default = "y")]
    drums: bool,                     // Drums activées
    #[serde(default = "y")]
    bass: bool,                      // Basse activée
    #[serde(default = "y")]
    arps: bool,                      // Arpèges/lead activés
    #[serde(default = "n")]
    nappes: bool,                    // Nappes activées
    #[serde(default = "s44")]
    sig: String,                     // Signature rythmique
    #[serde(default = "rk")]
    pattern: String,                 // Pattern drums
    #[serde(default = "i51")]
    inst_val: u16,                   // Instrument lead (défaut = 51 = Synth Strings 1)
    loop_enabled: Option<bool>,      // Boucle activée ?
    tracks: Option<Vec<MidiTrackCfg>>, // Configuration des pistes
    walking: Option<bool>,           // Walking bass ?
    #[serde(default = "mv127")]
    master_vol: u8,                  // Volume master (0-127, défaut 127)
    #[serde(default)]
    custom_notes: Vec<CustomNote>,    // Notes personnalisées du PianoRoll (optionnel)
    #[serde(default)]
    custom_channels: Vec<u8>,         // Canaux en mode PianoRoll (même vides) — les autres canaux jouent le mode classique
}

/// Réponse standardisée du serveur.
#[derive(Serialize)]
struct Rsp {
    status: String,
}

/// Requête POST /config — modifie la configuration en temps réel.
///
/// Tous les champs sont Option, on ne modifie que ce qui est présent.
#[derive(Deserialize)]
struct Cfg {
    drums: Option<bool>,
    bass: Option<bool>,
    arpeggios: Option<bool>,
    nappes: Option<bool>,
    pattern: Option<String>,
    tempo: Option<u16>,
    sig: Option<String>,
    instrument: Option<u16>,
    tracks: Option<Vec<MidiTrackCfg>>,
    walking: Option<bool>,
    master_vol: Option<u8>,
    use432: Option<bool>,
    loop_offset: Option<i32>,
    use_loops: Option<bool>,
    loop_name: Option<String>,
    loop_volume: Option<u8>,
}

// ─── Valeurs par défaut pour serde ──────────────────────────────────────

fn t120() -> u32 { 120 }
fn y() -> bool { true }
fn n() -> bool { false }
fn rk() -> String { "rock".to_string() }
fn s44() -> String { "4/4".to_string() }
fn i51() -> u16 { 51 }
fn nv() -> u8 { 100 }
fn nd() -> u64 { 400 }
fn mv127() -> u8 { 127 }

// ─── Notes depuis ChordEv ──────────────────────────────────────────────

/// Extrait les notes MIDI depuis un ChordEv (en ignorant les silences).
fn notes_from_ev(e: &ChordEv) -> Vec<u8> {
    let mut v = vec![];
    for n in &e.notes {
        if let Ok(x) = note_midi(n) {
            v.push(x);
        }
    }
    v
}

// ─── Routes ────────────────────────────────────────────────────────────

/// GET / — Page d'accueil.
///
/// En mode standalone (frontend embarqué), cette route n'est pas utilisée
/// car tout le routage frontend est géré par `frontend_embed::serve`.
/// En mode dev, elle sert le vieil index.html statique.
#[cfg(not(feature = "standalone"))]
async fn idx() -> impl IntoResponse {
    Html(include_str!("../static/index.html"))
}

/// POST /play — Lance la lecture live.
///
/// 1. Met à jour l'état live (tempo, pattern, signature, mutes)
/// 2. Si des tracks sont fournis, les applique
/// 3. Si des loops WAV sont actifs, lance la boucle correspondante
/// 4. Si une séquence est fournie → lance `play_seq` dans un thread
/// 5. Sinon si des notes sont fournies → lance `play_notes`
async fn play(State(s): State<AppState>, Json(b): Json<PlayReq>) -> impl IntoResponse {
    let lv = &s.live;

    // ── Config ──────────────────────────────────────────────────
    if b.tempo > 0 {
        lv.tempo.store(b.tempo as u16, std::sync::atomic::Ordering::Relaxed);
    }
    lv.sig.store(sig_code(&b.sig), std::sync::atomic::Ordering::Relaxed);
    lv.pattern.store(pat(&b.pattern), std::sync::atomic::Ordering::Relaxed);
    lv.stop.store(false, std::sync::atomic::Ordering::Relaxed);

    if let Some(w) = b.walking {
        lv.walking.store(w, std::sync::atomic::Ordering::Relaxed);
    }

    // Appliquer la configuration des pistes si fournie
    if let Some(ref t) = b.tracks {
        apply_tracks(lv, t);
    }

    // Si pas de tracks, utiliser les flags simples (drums, bass, arps, nappes)
    if b.tracks.is_none() {
        let mut tracks = lv.tracks.lock().unwrap();
        if let Some(t) = tracks.iter_mut().find(|t| t.channel == 0) {
            t.program.store(b.inst_val, std::sync::atomic::Ordering::Relaxed);
            t.mute.store(!b.arps, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(t) = tracks.iter_mut().find(|t| t.channel == 2) {
            t.mute.store(!b.bass, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(t) = tracks.iter_mut().find(|t| t.channel == 3) {
            t.mute.store(!b.nappes, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(t) = tracks.iter_mut().find(|t| t.channel == 9) {
            t.mute.store(!b.drums, std::sync::atomic::Ordering::Relaxed);
        }
    }

    let do_loop = b.loop_enabled.unwrap_or(false);

    // Déterminer la source des événements (seq ou sequence, pour compatibilité)
    let ev: &[ChordEv] = if !b.seq.is_empty() {
        &b.seq
    } else if !b.sequence.is_empty() {
        &b.sequence
    } else {
        &[]
    };

    // ── Lancer le thread audio ──────────────────────────────────
    let link = s.midi.lock().unwrap().clone(); // Option<MidiLink>
    if let Some(link) = link {
        let h2 = link.handle;

        // Lancer la boucle WAV (si active) AVANT la séquence MIDI
        let tempo_now = lv.tempo.load(std::sync::atomic::Ordering::Relaxed);
        let loop_active = lv.use_loops.load(std::sync::atomic::Ordering::Relaxed);
        if loop_active {
            let lname = lv.loop_name.lock().unwrap().clone();
            let name_opt = if lname.is_empty() { None } else { Some(lname.as_str()) };
            let lvol = lv.loop_volume.load(std::sync::atomic::Ordering::Relaxed);
            samples::set_volume(lvol);
            samples::play_loop(tempo_now, name_opt, lv.loop_offset.load(std::sync::atomic::Ordering::Relaxed));
        }

        // Séquence d'accords ou notes immédiates ?
        if !ev.is_empty() {
            let sq = ev.to_vec();
            let l = Arc::clone(lv);
            std::thread::spawn(move || {
                if let Ok(mut c) = h2.lock() {
                    play_seq(&mut c, &sq, &l, do_loop);
                }
            });
        } else if let Some(ref n) = b.notes {
            let v = n.clone();
            let l2 = Arc::clone(lv);
            std::thread::spawn(move || {
                let mv = l2.master_vol.load(std::sync::atomic::Ordering::Relaxed);
                if let Ok(mut c) = h2.lock() {
                    play_notes(&mut c, &v, mv);
                }
            });
        }
    }

    Json(Rsp {
        status: "ok".into(),
    })
}

/// POST /config — Modifie la configuration en temps réel.
async fn conf(State(s): State<AppState>, Json(b): Json<Cfg>) -> impl IntoResponse {
    let lv = &s.live;

    if let Some(ref t) = b.tracks {
        apply_tracks(lv, t);
    }

    if let Some(v) = b.drums {
        if let Some(t) = lv.tracks.lock().unwrap().iter_mut().find(|t| t.channel == 9) {
            t.mute.store(!v, std::sync::atomic::Ordering::Relaxed);
        }
    }
    if let Some(v) = b.bass {
        if let Some(t) = lv.tracks.lock().unwrap().iter_mut().find(|t| t.channel == 2) {
            t.mute.store(!v, std::sync::atomic::Ordering::Relaxed);
        }
    }
    if let Some(v) = b.arpeggios {
        if let Some(t) = lv.tracks.lock().unwrap().iter_mut().find(|t| t.channel == 0) {
            t.mute.store(!v, std::sync::atomic::Ordering::Relaxed);
        }
    }
    if let Some(v) = b.nappes {
        if let Some(t) = lv.tracks.lock().unwrap().iter_mut().find(|t| t.channel == 3) {
            t.mute.store(!v, std::sync::atomic::Ordering::Relaxed);
        }
    }

    if let Some(ref p) = b.pattern {
        lv.pattern.store(pat(p), std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(t) = b.tempo {
        lv.tempo.store(t, std::sync::atomic::Ordering::Relaxed);
        samples::set_current_tempo(t);
    }
    if let Some(ref sg) = b.sig {
        lv.sig.store(sig_code(sg), std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(iv) = b.instrument {
        if let Some(t) = lv.tracks.lock().unwrap().iter_mut().find(|t| t.channel == 0) {
            t.program.store(iv, std::sync::atomic::Ordering::Relaxed);
        }
    }
    if let Some(w) = b.walking {
        lv.walking.store(w, std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(m) = b.master_vol {
        lv.master_vol.store(m, std::sync::atomic::Ordering::Relaxed);
    }

    // Accordage 432Hz
    if let Some(u) = b.use432 {
        let was = lv.use432.swap(u, std::sync::atomic::Ordering::Relaxed);
        if was != u {
            let link = s.midi.lock().unwrap().clone();
            if let Some(link) = link {
                if let Ok(mut c) = link.handle.lock() {
                    for &ch in &[0u8, 2, 3, 4] {
                        pb(&mut c, ch, if u { 6881 } else { 8192 });
                    }
                }
            }
        }
    }

    // Boucle WAV drums
    if let Some(off) = b.loop_offset {
        lv.loop_offset.store(off, std::sync::atomic::Ordering::Relaxed);
        samples::update_offset(off);
    }
    if let Some(lo) = b.use_loops {
        lv.use_loops.store(lo, std::sync::atomic::Ordering::Relaxed);
        samples::set_use_loops(lo);
    }
    if let Some(ref n) = b.loop_name {
        *lv.loop_name.lock().unwrap() = n.clone();
    }
    if let Some(lv2) = b.loop_volume {
        lv.loop_volume.store(lv2, std::sync::atomic::Ordering::Relaxed);
        samples::set_volume(lv2);
    }

    Json(Rsp {
        status: "ok".into(),
    })
}

/// POST /stop — Arrête la lecture live.
async fn stop(State(s): State<AppState>) -> impl IntoResponse {
    s.live.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    samples::stop_loop();
    let link = s.midi.lock().unwrap().clone();
    if let Some(link) = link {
        if let Ok(mut c) = link.handle.lock() {
            rch(&mut c); // All Notes Off sur tous les canaux
        }
    }
    Json(serde_json::json!({"status": "stopped"}))
}

/// Construit les entrées du rendu classique : notes MIDI par accord,
/// durées en beats, et configuration complète (pattern, walking, sig,
/// tracks). Partagé entre `/render-wav` et `/render-notes`.
fn render_inputs(b: &PlayReq) -> (Vec<Vec<u8>>, Vec<f64>, render::RenderCfg) {
    let ev: &[ChordEv] = if !b.seq.is_empty() {
        &b.seq
    } else if !b.sequence.is_empty() {
        &b.sequence
    } else {
        &[]
    };

    let mut notes_arrays: Vec<Vec<u8>> = Vec::new();
    let mut beats: Vec<f64> = Vec::new();
    for e in ev {
        notes_arrays.push(notes_from_ev(e));
        beats.push(e.beats);
    }

    let tracks_cfg: Vec<render::TrackCfg> = if let Some(ref tcfg) = b.tracks {
        // Liste EXACTE des pistes actives envoyée par le frontend (dynamique :
        // ajout/suppression de pistes). Les canaux absents sont traités comme
        // muets par generate_notes (lookup `map_or(false)`).
        tcfg.iter().map(|tc| render::TrackCfg {
            channel: tc.channel,
            program: tc.program.unwrap_or(0),
            volume: tc.volume.unwrap_or(100),
            mute: tc.mute.unwrap_or(false),
        }).collect()
    } else {
        // Anciens clients / API directe : 5 rôles par défaut avec flags simples
        vec![
            render::TrackCfg { channel: 0, program: b.inst_val, volume: 60, mute: !b.arps },
            render::TrackCfg { channel: 2, program: 33, volume: 70, mute: !b.bass },
            render::TrackCfg { channel: 3, program: 48, volume: 60, mute: !b.nappes },
            render::TrackCfg { channel: 9, program: 1, volume: 90, mute: !b.drums },
            render::TrackCfg { channel: 4, program: 2, volume: 50, mute: false },
        ]
    };

    let rcfg = render::RenderCfg {
        tempo: b.tempo,
        pattern: b.pattern.clone(),
        walking: b.walking.unwrap_or(false),
        sig: b.sig.clone(),
        lead_inst: b.inst_val,
        tracks: tracks_cfg,
    };

    (notes_arrays, beats, rcfg)
}

/// POST /render-notes — notes générées par le mode classique (base PianoRoll).
///
/// Renvoie la liste des notes MIDI (channel, start_time, pitch, duration,
/// velocity — en beats) que le mode classique jouerait pour la séquence et
/// la configuration données. Le frontend s'en sert pour pré-remplir les
/// PianoRolls de chaque piste.
async fn render_notes(
    State(_s): State<AppState>,
    Json(b): Json<PlayReq>,
) -> impl IntoResponse {
    let (notes_arrays, beats, rcfg) = render_inputs(&b);
    let notes = render::generate_notes(&notes_arrays, &beats, &rcfg);
    axum::Json(serde_json::json!({ "notes": notes }))
}

/// POST /note — audition d'une note en direct (preview PianoRoll).
///
/// Joue immédiatement une note sur le canal demandé via FluidSynth
/// (note on + note off après `duration_ms`). Le program de la piste
/// configurée est appliqué avant la note (sauf drums, kit fixe).
#[derive(Deserialize)]
struct NoteReq {
    channel: u8,
    pitch: u8,
    #[serde(default = "nv")]
    velocity: u8,
    #[serde(default = "nd")]
    duration_ms: u64,
}

async fn note(State(s): State<AppState>, Json(b): Json<NoteReq>) -> impl IntoResponse {
    let prog = s.live.tracks.lock().unwrap()
        .iter()
        .find(|t| t.channel == b.channel)
        .map(|t| t.program.load(std::sync::atomic::Ordering::Relaxed));
    let ch = b.channel;
    let pitch = b.pitch;
    let vel = b.velocity.min(127);
    let dur = b.duration_ms.min(5000);
    std::thread::spawn(move || {
        // Program change (sauf drums : kit fixe)
        if let Some(p) = prog {
            if ch != 9 {
                midi_send(&s, &[0xC0 | ch, p as u8]);
            }
        }
        // Note On → Note Off après `dur` ms. Si l'envoi échoue
        // (connexion morte), midi_send reconnecte automatiquement.
        if midi_send(&s, &[0x90 | ch, pitch, vel]) {
            std::thread::sleep(std::time::Duration::from_millis(dur));
            midi_send(&s, &[0x90 | ch, pitch, 0]);
        }
    });
    Json(Rsp { status: "ok".into() })
}

/// POST /render-wav — Rendu batch d'une séquence en WAV.
async fn render_wav(
    State(s): State<AppState>,
    Json(b): Json<PlayReq>,
) -> impl IntoResponse {
    use axum::http::HeaderMap;

    let ev: &[ChordEv] = if !b.seq.is_empty() {
        &b.seq
    } else if !b.sequence.is_empty() {
        &b.sequence
    } else {
        &[]
    };

    if ev.is_empty() && b.custom_notes.is_empty() {
        return (StatusCode::BAD_REQUEST, "Séquence vide — rien à rendre").into_response();
    }

    let (notes_arrays, beats, rcfg) = render_inputs(&b);

    // ── Choix du mode de rendu ─────────────────────────────
    // Si des notes personnalisées (PianoRoll) sont fournies, on les
    // utilise directement. Sinon, rendu classique par accord.
    let (smf, total_beats) = if !b.custom_notes.is_empty() || !b.custom_channels.is_empty() {
        // Canaux en mode PianoRoll : même sans notes (pianoRoll vidé) → muets
        let custom_channels: std::collections::HashSet<u8> = b.custom_channels.iter()
            .copied()
            .chain(b.custom_notes.iter().map(|n| n.channel))
            .collect();

        // Notes classiques (toutes pistes) — on garde celles des canaux NON custom
        let classic = render::generate_notes(&notes_arrays, &beats, &rcfg);
        let mut merged: Vec<render::CustomNote> = classic.into_iter()
            .filter(|n| !custom_channels.contains(&n.channel))
            .collect();

        // Notes personnalisées du PianoRoll (canaux custom) — vélocité
        // scalée par le volume de la piste (le mute est filtré plus bas
        // dans generate_smf_from_custom)
        for cn in &b.custom_notes {
            let vol = rcfg.tracks.iter()
                .find(|t| t.channel == cn.channel)
                .map_or(127, |t| t.volume) as u32;
            let v = ((cn.velocity as u32 * vol) / 127).clamp(0, 127) as u8;
            merged.push(render::CustomNote {
                channel: cn.channel,
                start_time: cn.start_time,
                pitch: cn.pitch,
                duration: cn.duration,
                velocity: v,
            });
        }

        let tracks: Vec<render::TrackCfg> = rcfg.tracks.to_vec();
        let smf = render::generate_smf_from_custom(&merged, &tracks, b.tempo as u16);
        let tb = merged.iter()
            .map(|n| n.start_time + n.duration)
            .fold(0.0, f64::max);
        (smf, tb)
    } else {
        let smf = render::generate_smf_fmt0(&notes_arrays, &beats, &rcfg);
        let tb = beats.iter().sum();
        (smf, tb)
    };

    // Utiliser la SoundFont détectée automatiquement
    let sf_path = s.soundfont.as_deref().unwrap_or(
        "/usr/share/sounds/sf3/MuseScore_General_Full.sf3"
    );

    // Durée totale en secondes
    let duration_sec = total_beats * 60.0 / b.tempo.max(1) as f64;

    match render::render_wav(&smf, sf_path, duration_sec, b.master_vol) {
        Ok(wav) => {
            let mut headers = HeaderMap::new();
            headers.insert(
                "Content-Type",
                "audio/wav".parse().unwrap(),
            );
            (StatusCode::OK, headers, wav).into_response()
        }
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, e).into_response()
        }
    }
}

// ─── Main ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    println!("🚀 chordZIC backend — serveur de séquencement MIDI");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    // Détection automatique de la SoundFont
    let soundfont = find_soundfont();

    // Initialisation MIDI
    let midi = Arc::new(Mutex::new(init_midi().map(|(handle, port)| MidiLink { handle, port })));
    samples::init();

    use patterns::PAT_ROCK;
    use std::sync::atomic::{AtomicI32, AtomicU16, AtomicU8};
    use std::sync::Mutex;

    let state = AppState {
        midi,
        soundfont,
        live: Arc::new(Live {
            tracks: Mutex::new(vec![
                LiveTrack::new(0, 51, 60),
                LiveTrack::new(2, 33, 70),
                LiveTrack::new(3, 48, 60),
                LiveTrack::new(9, 1, 90),
                LiveTrack::new(4, 2, 50),
            ]),
            pattern: AtomicU8::new(PAT_ROCK),
            tempo: AtomicU16::new(120),
            stop: AtomicBool::new(false),
            sig: AtomicU16::new(44),
            walking: AtomicBool::new(false),
            master_vol: AtomicU8::new(127),
            use432: AtomicBool::new(false),
            loop_offset: AtomicI32::new(0),
            use_loops: AtomicBool::new(false),
            loop_name: Mutex::new(String::new()),
            loop_volume: AtomicU8::new(80),
        }),
    };

    async fn samples_list() -> impl IntoResponse {
        let data = samples::get_available();
        (StatusCode::OK, axum::Json(data))
    }

    let port = std::env::var("PORT").unwrap_or_else(|_| "4000".to_string());

    // ── Construction du routeur ───────────────────────────────────
    // En mode standalone, les routes du frontend sont servies par
    // `frontend_embed::serve`.  En mode dev, la route GET / sert
    // l'index.html statique (le frontend est servi par Vite sur :5176).
    let mut app = Router::new()
        .route("/play", post(play))
        .route("/config", post(conf))
        .route("/stop", post(stop))
        .route("/render-wav", post(render_wav))
        .route("/render-notes", post(render_notes))
        .route("/note", post(note))
        .route("/samples-list", get(samples_list))
        .route("/save", post(grilles::save_grille))
        .route("/grilles", get(grilles::list_grilles))
        .route("/grilles/:name", axum::routing::delete(grilles::delete_grille))
        .layer(CorsLayer::permissive())
        .with_state(state);

    #[cfg(feature = "standalone")]
    {
        // Mode standalone : le frontend embarqué gère toutes les routes
        // non-API (SPA routing + fichiers statiques)
        app = app.fallback(frontend_embed::serve);
        println!("\n   🎯 Mode standalone — frontend embarqué dans le binaire");
    }

    #[cfg(not(feature = "standalone"))]
    {
        // Mode dev : servir l'ancien index.html statique sur /
        app = app.route("/", get(idx));
    }

    println!(
        "\n📡 Serveur prêt sur http://0.0.0.0:{}",
        port
    );
    println!("   Routes :");
    println!("     GET  /              → page d'accueil (frontend React)");  
    println!("     POST /play          → lancer la séquence live");
    println!("     POST /config        → modifier la config live");
    println!("     POST /stop          → arrêter la lecture");
    println!("     POST /render-wav    → rendu WAV (batch)");
    println!("     POST /render-notes  → notes mode classique (PianoRoll)");
    println!("     POST /note          → audition note en direct (preview)");
    println!("     GET  /samples-list  → boucles WAV disponibles");
    println!("     POST /save          → sauvegarder une grille (JSON)");
    println!("     GET  /grilles       → lister les grilles");
    println!("     DELETE /grilles/<n> → supprimer une grille\n");

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}
