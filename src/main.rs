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
/// - `walking`  : génération de walking bass
/// - `grilles`  : sauvegarde/chargement des grilles en JSON
mod click;
mod dsp;
mod grilles;
mod midi;
mod patterns;
mod render;
mod samples;
mod walking;

use axum::{
    extract::State,
    http::{header, StatusCode},
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
    apply_tracks, init_midi, note_midi, play_seq, play_notes, rch, pb, cc, pc, no_mv, ChordEv, Live, LiveTrack,
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
    /// Génération de lecture MIDI (mode Navig) : chaque play incrémente,
    /// chaque thread vérifie que SA génération est toujours la courante avant
    /// de jouer → un stop/relance invalide proprement les anciennes lectures.
    midi_gen: Arc<std::sync::atomic::AtomicU64>,
    live: Arc<Live>,            // État live mutable partagé entre threads HTTP et audio
    soundfont: Option<String>,  // Chemin vers la SoundFont (pour render-wav)
    click: Arc<click::ClickState>, // Config du clic (mode rendu uniquement)
    /// Derniers WAV rendus pour la lecture SÉPARÉE (main, clic) + durée totale :
    /// un scrub (start_at) relance la lecture depuis l'offset SANS re-rendre
    /// (FluidSynth ≈ 3 s — le clic lane doit être instantané comme en MIDI).
    rendered_dual: Arc<Mutex<Option<(String, String, f64)>>>,
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
    #[serde(default)]
    click_in_render: bool,           // Clic intégré au rendu WAV (mode Navig)
    #[serde(default)]
    click_separate: bool,            // Clic séparé : réponse JSON {main_url, click_url}
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
    #[serde(default)]
    start_at: Option<f64>,            // Position de départ (beats) pour la lecture MIDI
    #[serde(default)]
    loop_start: Option<f64>,          // Locator gauche (beats) — intervalle de boucle [L, R[
    #[serde(default)]
    loop_end: Option<f64>,            // Locator droit (beats) — intervalle de boucle [L, R[
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

    Json(Rsp {
        status: "ok".into(),
    })
}

/// POST /stop — Arrête la lecture live.
async fn stop(State(s): State<AppState>) -> impl IntoResponse {
    s.live.stop.store(true, std::sync::atomic::Ordering::Relaxed);
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
            drums: tc.drums.unwrap_or(false),
            bank_msb: tc.bank_msb.unwrap_or(0),
            bank_lsb: tc.bank_lsb.unwrap_or(0),
            fx: tc.effects.unwrap_or_default(),
        }).collect()
    } else {
        // Anciens clients / API directe : 5 rôles par défaut avec flags simples
        vec![
            render::TrackCfg { channel: 0, program: b.inst_val, volume: 60, mute: !b.arps, bank_msb: 0, bank_lsb: 0, fx: Default::default(), drums: false },
            render::TrackCfg { channel: 2, program: 33, volume: 70, mute: !b.bass, bank_msb: 0, bank_lsb: 0, fx: Default::default(), drums: false },
            render::TrackCfg { channel: 3, program: 48, volume: 60, mute: !b.nappes, bank_msb: 0, bank_lsb: 0, fx: Default::default(), drums: false },
            render::TrackCfg { channel: 9, program: 1, volume: 90, mute: !b.drums, bank_msb: 0, bank_lsb: 0, fx: Default::default(), drums: false },
            render::TrackCfg { channel: 4, program: 2, volume: 50, mute: false, bank_msb: 0, bank_lsb: 0, fx: Default::default(), drums: false },
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
    let (prog, is_drum, bank_msb, bank_lsb) = {
        let tracks = s.live.tracks.lock().unwrap();
        match tracks.iter().find(|t| t.channel == b.channel) {
            Some(t) => (
                Some(t.program.load(std::sync::atomic::Ordering::Relaxed)),
                t.drums.load(std::sync::atomic::Ordering::Relaxed),
                t.bank_msb.load(std::sync::atomic::Ordering::Relaxed),
                t.bank_lsb.load(std::sync::atomic::Ordering::Relaxed),
            ),
            None => (None, false, 0, 0),
        }
    };
    let ch = b.channel;
    let pitch = b.pitch;
    let vel = b.velocity.min(127);
    let dur = b.duration_ms.min(5000);
    std::thread::spawn(move || {
        // Piste percussion : la note est jouée sur le canal drums natif (9)
        // — le kit sonne quel que soit le canal de saisie de la piste.
        let out_ch = if is_drum && ch != 9 { 9 } else { ch };
        if let Some(p) = prog {
            if out_ch != 9 {
                // Program change (instrument)
                midi_send(&s, &[0xC0 | out_ch, p as u8]);
            } else if bank_msb != 0 || bank_lsb != 0 {
                // Kit drums alternatif (banque choisie) : bank select + program
                midi_send(&s, &[0xB0 | 9, 0, bank_msb]);
                midi_send(&s, &[0xB0 | 9, 32, bank_lsb]);
                midi_send(&s, &[0xC0 | 9, p as u8]);
            }
        }
        // Note On → Note Off après `dur` ms. Si l'envoi échoue
        // (connexion morte), midi_send reconnecte automatiquement.
        if midi_send(&s, &[0x90 | out_ch, pitch, vel]) {
            std::thread::sleep(std::time::Duration::from_millis(dur));
            midi_send(&s, &[0x90 | out_ch, pitch, 0]);
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
    let (smf, total_beats, all_notes) = if !b.custom_notes.is_empty() || !b.custom_channels.is_empty() {
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
            let t = rcfg.tracks.iter().find(|t| t.channel == cn.channel);
            // Piste mutée ou absente → silencieuse (logique métier : mute partout)
            if t.map_or(true, |t| t.mute) {
                continue;
            }
            // Piste percussion hors canal 9 → redirigée vers le 9 (le kit)
            let is_drum = t.map_or(false, |t| t.drums);
            let out_ch = if is_drum && cn.channel != 9 { 9 } else { cn.channel };
            let vol = t.map_or(127, |t| t.volume) as u32;
            let v = ((cn.velocity as u32 * vol) / 127).clamp(0, 127) as u8;
            merged.push(render::CustomNote {
                channel: out_ch,
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
        (smf, tb, merged)
    } else {
        let smf = render::generate_smf_fmt0(&notes_arrays, &beats, &rcfg);
        let tb = beats.iter().sum();
        let classic = render::generate_notes(&notes_arrays, &beats, &rcfg);
        (smf, tb, classic)
    };

    // Utiliser la SoundFont détectée automatiquement
    let sf_path = s.soundfont.as_deref().unwrap_or(
        "/usr/share/sounds/sf3/MuseScore_General_Full.sf3"
    );

    // Durée totale en secondes
    let duration_sec = total_beats * 60.0 / b.tempo.max(1) as f64;

    // ── Rendu : simple (1 passe, rapide) ou par piste (si effets actifs) ──
    let has_fx = rcfg.tracks.iter().any(|t| !t.fx.is_off());
    let render_result = if has_fx {
        render::render_wav_mixed(&all_notes, &rcfg, sf_path, duration_sec, b.master_vol)
    } else {
        render::render_wav(&smf, sf_path, duration_sec, b.master_vol)
    };

    match render_result {
        Ok(mut wav) => {
            // ── Clic (mode Navig) ────────────────────────────────────
            // Deux modes au choix :
            //  - MIXÉ (« Dans le rendu ») : le clic est rendu à part puis
            //    MÉLANGÉ au WAV principal → synchro échantillon-parfaite.
            //  - SÉPARÉ (sortie dédiée) : 2 WAV (main + clic) écrits en
            //    temp → réponse JSON ; le serveur jouera le clic sur la
            //    sortie choisie (POST /navig-click-start) pendant que le
            //    navigateur joue le main.
            let click_cfg = click::load(&s.click);
            // Priorité aux flags EXPLICITES de la requête ; la config globale
            // (ClickControl) ne sert que de défaut. Avant : OR → une requête
            // click_in_render=true était ignorée si un out_device était posé
            // (réponse « séparé » au lieu d'un WAV mixé).
            let mix_click = if b.click_in_render {
                true
            } else if b.click_separate {
                false
            } else {
                click_cfg.in_render
            };
            let sep_click = if b.click_separate {
                true
            } else if b.click_in_render {
                false
            } else {
                click_cfg.out_device.is_some() && !click_cfg.in_render
            };
            if mix_click || sep_click {
                let bars = (sig_code(&b.sig) / 10).max(1) as u64;
                let click_smf = render::generate_click_smf(
                    b.tempo.max(1) as u16,
                    bars,
                    total_beats,
                    click_cfg.accent,
                    click_cfg.sound,
                );
                let cwav = match render::render_wav(&click_smf, sf_path, duration_sec, 127) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("   ⚠️ Clic : rendu clic échoué ({})", e);
                        Vec::new()
                    }
                };
                if !cwav.is_empty() {
                    if sep_click {
                        // Mode SÉPARÉ : 2 WAV en temp → le frontend récupère
                        // les URLs, joue le main, et déclenche le clic serveur.
                        let dir = std::env::temp_dir().join("chordj_rendered");
                        let _ = std::fs::create_dir_all(&dir);
                        let ts = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_nanos())
                            .unwrap_or(0);
                        let main_name = format!("navig_{}_main.wav", ts);
                        let click_name = format!("navig_{}_click.wav", ts);
                        let _ = std::fs::write(dir.join(&main_name), &wav);
                        let _ = std::fs::write(dir.join(&click_name), &cwav);
                        let json = serde_json::json!({
                            "main_url": format!("/rendered/{}", main_name),
                            "click_url": format!("/rendered/{}", click_name),
                            "duration_sec": duration_sec,
                        });
                        return (StatusCode::OK, axum::Json(json)).into_response();
                    } else {
                        // Mode MIXÉ : synchro parfaite par construction
                        let gain = (click_cfg.volume as f32 / 100.0) * 1.0;
                        match render::mix_wavs(&wav, &cwav, gain) {
                            Ok(mixed) => wav = mixed,
                            Err(e) => eprintln!("   ⚠️ Clic : mix échoué ({})", e),
                        }
                    }
                }
            }

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

// ─── Bounce multitrack (mode PostProd) ─────────────────────────────────

/// Fichier WAV temporaire d'une piste (bounce multitrack).
#[derive(Serialize)]
struct RenderedTrackFile {
    channel: u8,
    program: u16,
    url: String,
}

/// Réponse de `POST /render-tracks`.
#[derive(Serialize)]
struct RenderTracksResp {
    tracks: Vec<RenderedTrackFile>,
    duration_sec: f64,
    tempo: u16,
    sig: String,
    /// Gain master par défaut (normalisation au pic du mix Navig) : le
    /// frontend PostProd l'applique pour retrouver le même niveau qu'en
    /// mode Navig au premier Play, avant ajustement par les faders.
    master_gain: f64,
}

/// POST /render-tracks — Bounce multitrack (mode PostProd).
///
/// Même requête que `/render-wav` (sequence, tracks, custom_notes...), mais
/// renvoie UN WAV par piste active (avec ses effets MIDI appliqués) au lieu
/// d'un mix unique. Les WAVs sont écrits dans le répertoire temporaire et
/// servis par `GET /rendered/<file>` (URLs, pas de base64 : les boucles
/// longues rendraient le JSON trop lourd).
async fn render_tracks(
    State(s): State<AppState>,
    Json(b): Json<PlayReq>,
) -> impl IntoResponse {
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

    // Même construction des notes que /render-wav (classique + PianoRoll)
    let (total_beats, all_notes) = if !b.custom_notes.is_empty() || !b.custom_channels.is_empty() {
        let custom_channels: std::collections::HashSet<u8> = b.custom_channels.iter()
            .copied()
            .chain(b.custom_notes.iter().map(|n| n.channel))
            .collect();
        let classic = render::generate_notes(&notes_arrays, &beats, &rcfg);
        let mut merged: Vec<render::CustomNote> = classic.into_iter()
            .filter(|n| !custom_channels.contains(&n.channel))
            .collect();
        for cn in &b.custom_notes {
            let t = rcfg.tracks.iter().find(|t| t.channel == cn.channel);
            // Piste mutée ou absente → silencieuse (logique métier : mute partout)
            if t.map_or(true, |t| t.mute) {
                continue;
            }
            // Piste percussion hors canal 9 → redirigée vers le 9 (le kit)
            let is_drum = t.map_or(false, |t| t.drums);
            let out_ch = if is_drum && cn.channel != 9 { 9 } else { cn.channel };
            let vol = t.map_or(127, |t| t.volume) as u32;
            let v = ((cn.velocity as u32 * vol) / 127).clamp(0, 127) as u8;
            merged.push(render::CustomNote {
                channel: out_ch,
                start_time: cn.start_time,
                pitch: cn.pitch,
                duration: cn.duration,
                velocity: v,
            });
        }
        let tb = merged.iter().map(|n| n.start_time + n.duration).fold(0.0, f64::max);
        (tb, merged)
    } else {
        let tb = beats.iter().sum();
        (tb, render::generate_notes(&notes_arrays, &beats, &rcfg))
    };

    let sf_path = s.soundfont.as_deref().unwrap_or(
        "/usr/share/sounds/sf3/MuseScore_General_Full.sf3"
    );
    let duration_sec = total_beats * 60.0 / b.tempo.max(1) as f64;

    match render::render_tracks_individual(&all_notes, &rcfg, sf_path, duration_sec) {
        Ok(tracks) => {
            // Répertoire temporaire + purge des fichiers > 30 min
            let dir = std::env::temp_dir().join("chordj_rendered");
            let _ = std::fs::create_dir_all(&dir);
            if let Ok(rd) = std::fs::read_dir(&dir) {
                let now = std::time::SystemTime::now();
                for e in rd.flatten() {
                    let fresh = e.metadata()
                        .and_then(|m| m.modified())
                        .map(|t| now.duration_since(t).map(|d| d.as_secs() < 1800).unwrap_or(false))
                        .unwrap_or(false);
                    if !fresh {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);

            // Écrire chaque piste + calculer le pic du mix (pour master_gain)
            let mut files = Vec::new();
            let mut pic: f32 = 0.0;
            for rt in &tracks {
                let fname = format!("pp_{}_{}.wav", ts, rt.channel);
                let path = dir.join(&fname);
                if std::fs::write(&path, &rt.wav).is_err() {
                    return (StatusCode::INTERNAL_SERVER_ERROR, "Écriture du WAV impossible").into_response();
                }
                // Pic du mix (somme des pistes) pour la normalisation par défaut
                if let Ok(mut rd) = hound::WavReader::new(std::io::Cursor::new(&rt.wav)) {
                    let ch = rd.spec().channels as usize;
                    let s: Vec<i16> = rd.samples::<i16>().filter_map(|x| x.ok()).collect();
                    for i in 0..s.len() / ch.max(1) {
                        let l = s[i * ch] as f32 / 32768.0;
                        let r = if ch > 1 { s[i * ch + 1] as f32 / 32768.0 } else { l };
                        pic = pic.max(l.abs()).max(r.abs());
                    }
                }
                let prog = rcfg.tracks.iter()
                    .find(|t| t.channel == rt.channel)
                    .map(|t| t.program)
                    .unwrap_or(0);
                files.push(RenderedTrackFile {
                    channel: rt.channel,
                    program: prog,
                    url: format!("/rendered/{}", fname),
                });
            }

            let norm = if pic > 1e-6 { 0.5 / pic } else { 1.0 };
            let master_gain = norm as f64 * (b.master_vol as f64) / 127.0;

            axum::Json(RenderTracksResp {
                tracks: files,
                duration_sec,
                tempo: b.tempo.max(1) as u16,
                sig: b.sig.clone(),
                master_gain,
            })
            .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

/// GET /rendered/<file> — Sert un WAV temporaire produit par le bounce
/// multitrack (mode PostProd). Le nom est strictement validé (anti
/// path traversal : uniquement alphanumérique, point, tiret, underscore).
async fn serve_rendered(axum::extract::Path(file): axum::extract::Path<String>) -> impl IntoResponse {
    use axum::http::HeaderMap;
    if file.is_empty()
        || !file.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return (StatusCode::BAD_REQUEST, "Nom invalide").into_response();
    }
    let path = std::env::temp_dir().join("chordj_rendered").join(&file);
    match std::fs::read(&path) {
        Ok(data) => {
            let mut headers = HeaderMap::new();
            headers.insert("Content-Type", "audio/wav".parse().unwrap());
            (StatusCode::OK, headers, data).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "Fichier introuvable").into_response(),
    }
}

// ─── Piste de clic : endpoints ──────────────────────────────────────────

/// GET /audio-devices — liste les sorties audio disponibles (cpal).
async fn audio_devices(State(s): State<AppState>) -> impl IntoResponse {
    let devs = click::list_output_devices();
    let current = s.click.out_device.lock().unwrap().clone();
    (StatusCode::OK, axum::Json(serde_json::json!({ "devices": devs, "current": current })))
}

/// POST /audio-device — change la sortie audio globale (vide = défaut système).
#[derive(serde::Deserialize)]
struct AudioDeviceReq {
    #[serde(default)]
    device: String,
}

async fn audio_device(State(s): State<AppState>, Json(b): Json<AudioDeviceReq>) -> impl IntoResponse {
    let d = if b.device.is_empty() { None } else { Some(b.device) };
    *s.click.out_device.lock().unwrap() = d;
    (StatusCode::OK, axum::Json(serde_json::json!({ "ok": true })))
}

/// GET /midi-ports — liste les ports MIDI de sortie + le port courant.
async fn midi_ports(State(s): State<AppState>) -> impl IntoResponse {
    let ports = midi::list_ports();
    let current = s.midi.lock().unwrap().as_ref().map(|l| l.port.clone()).unwrap_or_default();
    (StatusCode::OK, axum::Json(serde_json::json!({ "ports": ports, "current": current })))
}

/// POST /midi-port — rebranche la sortie MIDI sur un port (par index).
#[derive(serde::Deserialize)]
struct MidiPortReq {
    index: usize,
}

async fn midi_port(State(s): State<AppState>, Json(b): Json<MidiPortReq>) -> impl IntoResponse {
    match midi::connect_port(b.index) {
        Some((handle, port)) => {
            *s.midi.lock().unwrap() = Some(MidiLink { handle, port: port.clone() });
            (StatusCode::OK, axum::Json(serde_json::json!({ "ok": true, "port": port }))).into_response()
        }
        None => (StatusCode::BAD_REQUEST, "Index de port MIDI invalide").into_response(),
    }
}

/// GET /click — config de la piste de clic (mode Navig).
async fn get_click(State(s): State<AppState>) -> impl IntoResponse {
    let c = click::load(&s.click);
    let cfg = serde_json::json!({
        "volume": c.volume,
        "accent": c.accent,
        "sound": c.sound,
        "sound_name": click::sound_name(c.sound),
        "in_render": c.in_render,
        "out_device": c.out_device,
        "delay_ms": c.delay_ms,
        "sounds": [
            { "id": 0, "name": "Métronome GM" },
            { "id": 1, "name": "Woodblock" },
            { "id": 2, "name": "Agogo" },
            { "id": 3, "name": "Taiko" },
        ],
    });
    (StatusCode::OK, axum::Json(cfg))
}

#[derive(serde::Deserialize)]
struct ClickReq {
    volume: Option<u8>,
    accent: Option<bool>,
    sound: Option<u8>,
    in_render: Option<bool>,
    out_device: Option<String>,
    delay_ms: Option<i32>,
}

/// POST /click — met à jour la config de la piste de clic.
async fn post_click(State(s): State<AppState>, Json(b): Json<ClickReq>) -> impl IntoResponse {
    let st = &s.click;
    if let Some(v) = b.volume {
        st.volume.store(v.min(100), std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(v) = b.accent {
        st.accent.store(v, std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(v) = b.sound {
        st.sound.store(v.min(3), std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(v) = b.in_render {
        st.in_render.store(v, std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(d) = b.out_device {
        let d2 = if d.is_empty() { None } else { Some(d) };
        *st.out_device.lock().unwrap() = d2;
    }
    if let Some(v) = b.delay_ms {
        st.delay_ms.store(v.clamp(-200, 200), std::sync::atomic::Ordering::Relaxed);
    }
    (StatusCode::OK, axum::Json(serde_json::json!({ "ok": true })))
}

#[derive(serde::Deserialize)]
struct ClickStartReq {
    file: String,           // nom du fichier /rendered/<file> (clic)
    device: Option<String>, // sortie dédiée (None = défaut)
    start_in_ms: u64,       // délai avant lecture (handshake avec le navigateur)
}

/// POST /navig-click-start — le serveur joue le clic séparé sur la sortie
/// choisie, synchronisé avec le démarrage du navigateur (start_in_ms).
async fn navig_click_start(State(s): State<AppState>, Json(b): Json<ClickStartReq>) -> impl IntoResponse {
    let dir = std::env::temp_dir().join("chordj_rendered");
    let path = dir.join(&b.file);
    if !path.exists() {
        return (StatusCode::NOT_FOUND, "Clic introuvable").into_response();
    }
    let delay = s.click.delay_ms.load(std::sync::atomic::Ordering::Relaxed);
    match click::play_click_wav(path.to_str().unwrap_or(""), b.device, b.start_in_ms, delay) {
        Ok(()) => (StatusCode::OK, axum::Json(serde_json::json!({ "ok": true }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Clic : {}", e)).into_response(),
    }
}

/// POST /navig-click-stop — arrête le clic séparé (ou la lecture double canaux).
async fn navig_click_stop() -> impl IntoResponse {
    click::stop_click();
    click::stop_dual();
    (StatusCode::OK, axum::Json(serde_json::json!({ "ok": true })))
}

/// POST /navig-play-midi — lecture MIDI temps réel (mode Navig) : toutes les
/// pistes (grille + notes personnalisées du PianoRoll) jouées sur le port MIDI
/// courant (ex. Roland), comme le mode Live mais avec le contenu du Navig.
async fn navig_play_midi(State(s): State<AppState>, Json(b): Json<PlayReq>) -> impl IntoResponse {
    let ev: &[ChordEv] = if !b.seq.is_empty() {
        &b.seq
    } else if !b.sequence.is_empty() {
        &b.sequence
    } else {
        &[]
    };
    if ev.is_empty() && b.custom_notes.is_empty() {
        return (StatusCode::BAD_REQUEST, "Séquence vide — rien à jouer").into_response();
    }
    let (notes_arrays, beats, rcfg) = render_inputs(&b);
    let all_notes: Vec<render::CustomNote> = if !b.custom_notes.is_empty() || !b.custom_channels.is_empty() {
        let custom_channels: std::collections::HashSet<u8> = b.custom_channels
            .iter()
            .copied()
            .chain(b.custom_notes.iter().map(|n| n.channel))
            .collect();
        let classic = render::generate_notes(&notes_arrays, &beats, &rcfg);
        let mut merged: Vec<render::CustomNote> = classic
            .into_iter()
            .filter(|n| !custom_channels.contains(&n.channel))
            .collect();
        for cn in &b.custom_notes {
            let t = rcfg.tracks.iter().find(|t| t.channel == cn.channel);
            // Piste mutée ou absente → silencieuse (logique métier : mute partout)
            if t.map_or(true, |t| t.mute) {
                continue;
            }
            // Piste percussion hors canal 9 → redirigée vers le 9 (le kit)
            let is_drum = t.map_or(false, |t| t.drums);
            let out_ch = if is_drum && cn.channel != 9 { 9 } else { cn.channel };
            let vol = t.map_or(127, |t| t.volume) as u32;
            let v = ((cn.velocity as u32 * vol) / 127).clamp(0, 127) as u8;
            merged.push(render::CustomNote {
                channel: out_ch,
                start_time: cn.start_time,
                pitch: cn.pitch,
                duration: cn.duration,
                velocity: v,
            });
        }
        merged
    } else {
        render::generate_notes(&notes_arrays, &beats, &rcfg)
    };
    if all_notes.is_empty() {
        return (StatusCode::BAD_REQUEST, "Aucune note à jouer").into_response();
    }

    let midi = s.midi.clone();
    let guard = midi.lock().unwrap();
    match guard.as_ref() {
        Some(link) => {
            // Nouvelle génération de lecture : invalide les threads précédents
            let gen = s.midi_gen.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            let start_at = b.start_at.unwrap_or(0.0);
            // Durée totale en beats (pour le clic métronome MIDI)
            let total_beats = all_notes.iter().map(|n| n.start_time + n.duration).fold(0.0, f64::max);
            // Intervalle de boucle (locators [L, R[ en beats) : le repeat
            // boucle [L, R[ au lieu du morceau complet. Invalide → complet.
            let loop_start = b.loop_start.unwrap_or(0.0);
            let loop_end = b.loop_end.unwrap_or(0.0);
            let loop_end = if loop_end > loop_start + 0.01 { loop_end } else { total_beats };
            midi_play_custom(
                link.handle.clone(),
                all_notes.clone(),
                rcfg.tracks.to_vec(),
                b.tempo,
                start_at,
                gen,
                s.midi_gen.clone(),
                s.click.clone(),
                sig_code(&b.sig),
                total_beats,
                b.master_vol,
                b.loop_enabled.unwrap_or(false),
                loop_start,
                loop_end,
            );
            drop(guard);
            let dur = all_notes.iter().map(|n| n.start_time + n.duration).fold(0.0, f64::max)
                * 60.0 / b.tempo.max(1) as f64;
            (StatusCode::OK, axum::Json(serde_json::json!({ "ok": true, "notes": all_notes.len(), "duration_sec": dur.round() }))).into_response()
        }
        None => (StatusCode::INTERNAL_SERVER_ERROR, "Aucun port MIDI connecté").into_response(),
    }
}

/// POST /navig-stop-midi — arrête la lecture MIDI en cours : invalide la
/// génération courante (les threads en attente s'arrêtent) + coupe le son
/// (CC120 All Sound Off + CC123 All Notes Off + CC121 Reset Controllers).
async fn navig_stop_midi(State(s): State<AppState>) -> impl IntoResponse {
    s.midi_gen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    for ch in 0..16u8 {
        // All Sound Off — coupe même les notes avec réverb/delay qui prolongent
        midi_send(&s, &[0xB0 | ch, 120, 0]);
        // All Notes Off
        midi_send(&s, &[0xB0 | ch, 123, 0]);
        // Reset All Controllers
        midi_send(&s, &[0xB0 | ch, 121, 0]);
    }
    (StatusCode::OK, axum::Json(serde_json::json!({ "ok": true })))
}

/// Joue des notes (mode Navig) en MIDI temps réel sur la connexion donnée :
/// reset + program changes des pistes (avec sends FX), puis note-on/off
/// programmés au bon timing (start_time/duration en beats, tempo).
/// `start_at` : position de départ en beats (les notes antérieures sont ignorées).
/// `gen`/`gen_ref` : génération de lecture — un thread ne joue que si SA
/// génération est toujours la courante (stop/relance → invalidation propre).
fn midi_play_custom(
    handle: midi::MidiHandle,
    notes: Vec<render::CustomNote>,
    tracks: Vec<render::TrackCfg>,
    tempo: u32,
    start_at: f64,
    gen: u64,
    gen_ref: Arc<std::sync::atomic::AtomicU64>,
    click: Arc<click::ClickState>,
    sig_code_v: u16,
    total_beats: f64,
    master_vol: u8,
    loop_playback: bool,
    loop_start: f64,
    loop_end: f64,
) {
    std::thread::spawn(move || {
        use std::sync::atomic::Ordering;
        // ── Logique métier du clic ──
        // Le clic est le MÉTRONOME du serveur : il sort par FLUID SYNTH
        // (connexion MIDI séparée, son GM fiable 33/34), JAMAIS par
        // l'instrument du morceau (le Roland) — le Roland ne reçoit que les
        // notes, le clic reste indépendant et contrôlable en direct.
        // Repli si FluidSynth absent : canal libre du port principal.
        let fluid = midi::open_by_name("fluid");
        let used: std::collections::HashSet<u8> = tracks.iter().map(|t| t.channel).collect();
        let click_chan = (0u8..16).find(|c| *c != 9 && !used.contains(c)).unwrap_or(9);
        // Repli sans FluidSynth : mêmes hauteurs que le rendu WAV (72/74/55).
        // Le métronome GM (33/34) n'existe que sur le canal drums : si un
        // canal drums est libre → GM natif, sinon repli woodblock (le plus
        // proche). Programme appliqué au setup ci-dessous.
        let (click_pitch, click_prog) = if click_chan == 9 {
            (34u8, 0u16) // canal drums libre → métronome GM
        } else {
            match click.sound.load(Ordering::Relaxed) {
                click::SOUND_WOODBLOCK => (72u8, 115u16),
                click::SOUND_AGOGO => (74u8, 114u16),
                click::SOUND_TAIKO => (55u8, 116u16),
                _ => (72u8, 115u16), // métronome GM → woodblock en repli
            }
        };
        // 1. Setup : reset + program changes + sends d'effets
        {
            let mut c = handle.lock().unwrap();
            rch(&mut c);
            for t in &tracks {
                if t.mute {
                    continue;
                }
                let ch = t.channel;
                if t.drums && ch != 9 {
                    continue; // piste percussion → redirigée vers le canal 9
                }
                if ch == 9 {
                    // Kit drums : bank select si un kit alternatif est choisi
                    if t.bank_msb != 0 || t.bank_lsb != 0 {
                        cc(&mut c, 9, 0, t.bank_msb);
                        cc(&mut c, 9, 32, t.bank_lsb);
                    }
                    pc(&mut c, 9, t.program as u8);
                } else {
                    pc(&mut c, ch, t.program as u8);
                    cc(&mut c, ch, 91, t.fx.reverb as u8);
                    cc(&mut c, ch, 93, t.fx.chorus as u8);
                }
            }
            // Program change de l'instrument de percussion du clic —
            // uniquement en REPLI (port principal) : FluidSynth utilise
            // le canal 9 drums GM natif, aucun réglage nécessaire.
            if fluid.is_none() && click_chan != 9 {
                pc(&mut c, click_chan, click_prog as u8);
            }
        }
        // Clic sur FluidSynth : sons mélodiques → program change canal 15
        if let Some(f) = &fluid {
            let snd = click.sound.load(Ordering::Relaxed);
            if snd != click::SOUND_GM_METRONOME {
                let mut fc = f.lock().unwrap();
                match snd {
                    click::SOUND_WOODBLOCK => pc(&mut fc, 15, 115),
                    click::SOUND_AGOGO => pc(&mut fc, 15, 114),
                    click::SOUND_TAIKO => pc(&mut fc, 15, 116),
                    _ => {}
                }
            }
        }
        // 2. Notes programmées + clic — SCHEDULER MONO-THREAD
        // Une lecture = UN thread + une min-heap d'événements (note-on/off +
        // clic). Avant : 1 thread PAR NOTE → les grilles énormes (4000+
        // notes) créaient des milliers de threads → limite système atteinte
        // → spawn en échec (Resource temporarily unavailable) → notes et
        // clic absents (surtout au repeat : les lectures non stoppées
        // s'accumulaient). Horloge UNIQUE (start.elapsed()) → clic et notes
        // parfaitement synchrones, même en boucle et après un scrub.
        let tempo_ms = 60_000.0 / tempo.max(1) as f64;
        // Intervalle de boucle (locators [L, R[ en beats) : le repeat boucle
        // [L, R[ au lieu du morceau complet. Sans intervalle valide :
        // boucle complète (loop_start = 0, loop_end = total_beats).
        let loop_end = if loop_end > loop_start + 0.01 { loop_end } else { total_beats };
        let loop_len_ms = (loop_end - loop_start).max(0.0) * tempo_ms; // durée d'un cycle
        // Durée de la passe 0 (depuis start_at) : après un scrub, la 1re
        // passe est plus courte que l'intervalle ; les cycles suivants
        // durent loop_len_ms — même référentiel que le WAV.
        let pass0_len_ms = ((loop_end - start_at).max(0.0)) * tempo_ms;
        // En BOUCLE, TOUTES les notes de l'intervalle sont gardées (les
        // passes suivantes repartent de L) ; sans boucle, on ignore celles
        // avant start_at. Les notes après le locator droit ne sont jamais
        // jouées.
        let sorted = select_notes(notes, loop_playback, start_at, loop_start, loop_end);
        // Clic : sortie et hauteurs (FluidSynth si dispo, sinon repli).
        let bars = (sig_code_v / 10).max(1) as u64;
        let total_int = total_beats.ceil() as u64;
        let (clk, cch, pitch_acc, pitch_norm) = match &fluid {
            Some(f) => match click.sound.load(Ordering::Relaxed) {
                click::SOUND_WOODBLOCK => (f.clone(), 15u8, 72u8, 72u8),
                click::SOUND_AGOGO => (f.clone(), 15u8, 74u8, 74u8),
                click::SOUND_TAIKO => (f.clone(), 15u8, 55u8, 55u8),
                _ => (f.clone(), 9u8, 34u8, 33u8), // métronome GM
            },
            None => {
                if click_chan == 9 {
                    (handle.clone(), 9u8, 34u8, 33u8) // métronome GM natif
                } else {
                    (handle.clone(), click_chan, click_pitch, click_pitch) // repli
                }
            }
        };
        // État par note : canal de sortie, durée brute, passe courante.
        let note_out: Vec<u8> = sorted.iter().map(|n| {
            let is_drum = tracks.iter().any(|t| t.channel == n.channel && t.drums);
            if is_drum && n.channel != 9 { 9 } else { n.channel }
        }).collect();
        let note_dur: Vec<u64> = sorted.iter()
            .map(|n| ((n.duration * tempo_ms).max(60.0)) as u64)
            .collect();
        let mut note_pass: Vec<u64> = sorted.iter()
            .map(|n| if n.start_time + n.duration > start_at { 0 } else { 1 })
            .collect();

        // Min-heap d'événements : (temps en µs, séquence, événement).
        // BinaryHeap = max-heap → on stocke l'opposé du temps.
        #[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
        enum Ev { NoteOn(usize), NoteOff(usize), ClickOn, ClickOff }
        let mut heap: std::collections::BinaryHeap<(i64, u64, Ev)> = std::collections::BinaryHeap::new();
        let mut seq: u64 = 0;
        macro_rules! push_ev {
            ($t_ms:expr, $ev:expr) => {{
                seq += 1;
                let t_us = (($t_ms) * 1000.0).round() as i64;
                heap.push((-t_us, seq, $ev));
            }};
        }
        // Planifie le note-on d'une note pour sa passe courante (skip les
        // passes où la note est entièrement hors passe ; une note avant le
        // locator gauche n'est plus jouée après la passe 0).
        macro_rules! plan_note {
            ($i:expr) => {{
                let mut done = false;
                while !done {
                    let n = &sorted[$i];
                    let pass = note_pass[$i];
                    if pass >= 1 && n.start_time < loop_start {
                        done = true; // hors intervalle : plus rien à jouer
                    } else {
                        let (target_ms, end_ms) = note_loop_window(pass, n.start_time, start_at, loop_start, loop_end, tempo_ms);
                        let dur_eff = note_off_clamped(target_ms, end_ms, note_dur[$i] as f64, loop_playback);
                        if dur_eff <= 0.0 {
                            if !loop_playback {
                                done = true; // plus rien à jouer pour cette note
                            } else {
                                note_pass[$i] += 1; // note hors passe → suivante
                            }
                        } else {
                            push_ev!(target_ms, Ev::NoteOn($i));
                            done = true;
                        }
                    }
                }
            }};
        }
        // État du clic : beat courant, cycle (passe) courant, offset du cycle.
        let mut b: u64 = start_at.ceil() as u64;
        let mut cycle: u64 = 0;
        let mut offset_ms = 0.0f64;
        let mut last_click_pitch: u8 = pitch_norm;
        let dur_click_ms = (0.15 * tempo_ms) as u64;
        // Planifie le prochain clic (b déjà avancé par l'appelant si besoin).
        // Le décalage est lu à la planification (comme l'ancien code).
        macro_rules! plan_click {
            () => {{
                let (nb, nc, no) = next_click_state(
                    b, cycle, offset_ms, loop_playback, loop_start, loop_end,
                    pass0_len_ms, loop_len_ms, total_int,
                );
                // Fin de lecture (non bouclée) : plus de clic à planifier.
                if loop_playback || nb <= total_int {
                    b = nb;
                    cycle = nc;
                    offset_ms = no;
                    let shift_ms = click.delay_ms.load(Ordering::Relaxed) as f64;
                    let beat_in_cycle = if cycle == 0 { b as f64 - start_at } else { b as f64 - loop_start };
                    let target_ms = (offset_ms + beat_in_cycle * tempo_ms + shift_ms).max(0.0);
                    push_ev!(target_ms, Ev::ClickOn);
                }
            }};
        }

        let start = std::time::Instant::now();
        let now_ms = || start.elapsed().as_secs_f64() * 1000.0;
        // Initialisation : premiers note-on + premier clic.
        for i in 0..sorted.len() {
            plan_note!(i);
        }
        plan_click!();

        // Boucle d'horloge : joue les événements dus, dort jusqu'au suivant.
        loop {
            if gen_ref.load(Ordering::Relaxed) != gen {
                return; // lecture invalidée (stop/relance)
            }
            let now = now_ms();
            // Joue tous les événements dont l'heure est arrivée (tolérance 0,5 ms).
            while let Some((neg_t_us, _, kind)) = heap.peek().copied() {
                let t_ms = (-neg_t_us) as f64 / 1000.0;
                if t_ms > now + 0.5 {
                    break;
                }
                heap.pop();
                match kind {
                    Ev::NoteOn(i) => {
                        let n = &sorted[i];
                        let (target_ms, end_ms) = note_loop_window(note_pass[i], n.start_time, start_at, loop_start, loop_end, tempo_ms);
                        let dur_eff = note_off_clamped(target_ms, end_ms, note_dur[i] as f64, loop_playback);
                        {
                            let mut c = handle.lock().unwrap();
                            no_mv(&mut c, note_out[i], n.pitch, n.velocity, master_vol);
                        }
                        push_ev!(t_ms + dur_eff, Ev::NoteOff(i));
                    }
                    Ev::NoteOff(i) => {
                        let n = &sorted[i];
                        {
                            let mut c = handle.lock().unwrap();
                            no_mv(&mut c, note_out[i], n.pitch, 0, 127);
                        }
                        if loop_playback {
                            note_pass[i] += 1;
                            plan_note!(i);
                        }
                    }
                    Ev::ClickOn => {
                        let vol = click.volume.load(Ordering::Relaxed);
                        if vol == 0 {
                            // Muté : pas de note-on ; le prochain clic suit
                            // directement (pas de note-off orphelin).
                            b += 1;
                            plan_click!();
                        } else {
                            // Accent : beat LOCAL du cycle (0 = début du
                            // morceau) — les accents restent sur les débuts
                            // de mesure à CHAQUE cycle.
                            let acc = click.accent.load(Ordering::Relaxed) && b % bars == 0;
                            let vel = (vol as f32 / 100.0 * if acc { 127.0 } else { 120.0 }).round() as u8;
                            let pitch = if acc { pitch_acc } else { pitch_norm };
                            last_click_pitch = pitch;
                            {
                                let mut c = clk.lock().unwrap();
                                no_mv(&mut c, cch, pitch, vel, 127);
                            }
                            push_ev!(t_ms + dur_click_ms as f64, Ev::ClickOff);
                        }
                    }
                    Ev::ClickOff => {
                        {
                            let mut c = clk.lock().unwrap();
                            no_mv(&mut c, cch, last_click_pitch, 0, 127);
                        }
                        b += 1;
                        plan_click!();
                    }
                }
            }
            // Fin de lecture (non bouclée) : plus aucun événement → terminé.
            if heap.is_empty() && !loop_playback {
                break;
            }
            // Sommeil : jusqu'au prochain événement, plafonné à 5 ms
            // (réactivité du stop/relance).
            let next_t = heap.peek().map(|&(neg, _, _)| (-neg) as f64 / 1000.0).unwrap_or(now + 5.0);
            let sleep_ms = ((next_t - now).clamp(0.0, 5.0)) as u64;
            if sleep_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
            }
        }
        println!("🎹 Lecture MIDI Navig terminée : {} notes", sorted.len());
    });
}

/// Convertit start_at (beats) → secondes pour navig-play : offset de lecture
/// du rendu WAV (main + clic) quand l'utilisateur déplace la tête de lecture.
fn start_sec_from_beats(start_at: Option<f64>, tempo: u32) -> f64 {
    match start_at {
        Some(ba) if ba > 0.0 => (ba * 60.0) / tempo.max(1) as f64,
        _ => 0.0,
    }
}

/// Sélection + tri des notes pour une lecture MIDI (boucle [L, R[).
/// - Les notes dont le début est ≥ `loop_end` ne sont JAMAIS jouées.
/// - Sans boucle : ignore les notes finissant avant `start_at` (scrub).
/// - Avec boucle : toutes les notes < `loop_end` sont gardées (les passes
///   suivantes repartent de L). Tri par start_time pour le scheduler.
fn select_notes(
    notes: Vec<render::CustomNote>,
    loop_playback: bool,
    start_at: f64,
    _loop_start: f64,
    loop_end: f64,
) -> Vec<render::CustomNote> {
    let mut sorted: Vec<_> = notes
        .into_iter()
        .filter(|n| n.start_time < loop_end)
        .filter(|n| loop_playback || n.start_time + n.duration > start_at)
        .collect();
    sorted.sort_by(|a, b| a.start_time.partial_cmp(&b.start_time).unwrap_or(std::cmp::Ordering::Equal));
    sorted
}

/// Durée effective (ms) d'une note à la passe `pass` — clamp du note-off.
/// En boucle, le note-off est clampé à la fin de la passe (−2 ms) pour que
/// l'off passe TOUJOURS avant le note-on du cycle suivant : sinon le vieux
/// off coupe la même note rejouée au début du passage suivant (bug historique
/// « le son coupe au 2e passage », en MIDI le off tue la note de même
/// canal/pitch). Sans boucle : durée brute inchangée.
fn note_off_clamped(target_ms: f64, end_ms: f64, note_dur_ms: f64, loop_playback: bool) -> f64 {
    if loop_playback {
        (end_ms - target_ms - 2.0).min(note_dur_ms).max(0.0)
    } else {
        note_dur_ms
    }
}

/// État suivant du clic métronome — wrap sur l'intervalle [L, R[.
/// Quand le beat courant atteint `loop_end`, on revient à `loop_start`
/// (le beat `loop_end` n'est PAS joué : il coïnciderait avec le beat L du
/// cycle suivant → double clic) et le cycle avance (offset = fin de la
/// passe 0 + (cycle−1) × durée d'un cycle). Sans boucle, une fois le
/// morceau dépassé (`b > total_int`) plus aucun clic n'est planifié
/// (état renvoyé inchangé).
fn next_click_state(
    b: u64,
    cycle: u64,
    offset_ms: f64,
    loop_playback: bool,
    loop_start: f64,
    loop_end: f64,
    pass0_len_ms: f64,
    loop_len_ms: f64,
    total_int: u64,
) -> (u64, u64, f64) {
    if !loop_playback && b > total_int {
        return (b, cycle, offset_ms); // fin de lecture : plus de clic
    }
    if loop_playback && b as f64 >= loop_end {
        let b = loop_start.floor() as u64;
        let cycle = cycle + 1;
        let offset_ms = pass0_len_ms + (cycle - 1) as f64 * loop_len_ms;
        return (b, cycle, offset_ms);
    }
    (b, cycle, offset_ms)
}

/// Fenêtre temporelle (ms) d'une note à la passe `pass` d'une lecture MIDI
/// en BOUCLE : renvoie (cible du note-on, fin de passe pour le note-off).
/// La boucle couvre l'intervalle [loop_start, loop_end[ (locators) — par
/// défaut [0, total_beats[ = morceau complet. Passe 0 = depuis start_at
/// (fin = pass0_len) ; passes ≥ 1 = intervalle complet (chaque cycle dure
/// loop_len, le 1er commence à la fin de la passe 0 — après un scrub, la
/// boucle reboucle dès la fin du restant, comme la lecture WAV). Sans
/// boucle, seule la passe 0 est jouée.
fn note_loop_window(
    pass: u64,
    start_time: f64,
    start_at: f64,
    loop_start: f64,
    loop_end: f64,
    tempo_ms: f64,
) -> (f64, f64) {
    let loop_len = (loop_end - loop_start).max(0.0) * tempo_ms;
    let pass0_len = (loop_end - start_at).max(0.0) * tempo_ms;
    let offset = if pass == 0 { 0.0 } else { pass0_len + (pass - 1) as f64 * loop_len };
    let within = if pass == 0 {
        (start_time - start_at).max(0.0) * tempo_ms
    } else {
        (start_time - loop_start) * tempo_ms
    };
    let end = offset + if pass == 0 { pass0_len } else { loop_len };
    (offset + within, end)
}

/// POST /navig-play — lecture SERVEUR du rendu en double canaux :
/// le WAV principal (canaux 1-2) + le clic (canaux 3-4) sur un appareil
/// MULTICANAL (agrégat CoreAudio) → UNE seule horloge, synchro
/// échantillon-parfaite entre les deux sorties physiques.
async fn navig_play(State(s): State<AppState>, Json(b): Json<PlayReq>) -> impl IntoResponse {
    let ev: &[ChordEv] = if !b.seq.is_empty() {
        &b.seq
    } else if !b.sequence.is_empty() {
        &b.sequence
    } else {
        &[]
    };
    if ev.is_empty() && b.custom_notes.is_empty() {
        return (StatusCode::BAD_REQUEST, "Séquence vide — rien à jouer").into_response();
    }

    // ── Scrub (start_at) : relance INSTANTANÉE depuis l'offset ──
    // L'utilisateur a déplacé la tête de lecture pendant la lecture. On
    // réutilise le rendu déjà fait (fichiers + durée en cache) → pas de
    // FluidSynth : le clic lane saute aussitôt, comme en lecture MIDI.
    // (Le rendu complet ≈ 3 s : re-rendre à chaque clic donnait l'impression
    // que « la lecture continue » sans jamais sauter.)
    let start_sec = start_sec_from_beats(b.start_at, b.tempo);
    let loop_playback = b.loop_enabled.unwrap_or(false);
    // Intervalle de boucle (locators [L, R[ en beats) → secondes. Si
    // l'intervalle est invalide ou absent : boucle complète (0 → fin).
    let (loop_start_sec, loop_end_sec) = if loop_playback {
        let ls = b.loop_start.unwrap_or(0.0);
        let le = b.loop_end.unwrap_or(0.0);
        if le > ls + 0.01 {
            (ls * 60.0 / b.tempo.max(1) as f64, le * 60.0 / b.tempo.max(1) as f64)
        } else {
            (0.0, 0.0)
        }
    } else {
        (0.0, 0.0)
    };
    if start_sec > 0.0 {
        let cache = s.rendered_dual.lock().unwrap().clone();
        if let Some((mp, cp, dur)) = cache {
            if std::path::Path::new(&mp).exists() && std::path::Path::new(&cp).exists() {
                let cfg = click::load(&s.click);
                if let Some(d) = cfg.out_device.filter(|d| !d.is_empty()) {
                    match click::play_dual(&mp, &cp, &d, s.click.clone(), start_sec, loop_playback, loop_start_sec, loop_end_sec) {
                        Ok(()) => {
                            return (StatusCode::OK, axum::Json(serde_json::json!({
                                "ok": true,
                                "mode": "channels",
                                "duration_sec": (dur - start_sec).max(0.0),
                                "total_duration_sec": dur,
                            }))).into_response();
                        }
                        Err(_) => { /* repli : rendu complet ci-dessous (rare) */ }
                    }
                }
            }
        }
    }

    // 1. Rendu du WAV principal (sans clic)
    let ev: &[ChordEv] = if !b.seq.is_empty() {
        &b.seq
    } else if !b.sequence.is_empty() {
        &b.sequence
    } else {
        &[]
    };
    if ev.is_empty() && b.custom_notes.is_empty() {
        return (StatusCode::BAD_REQUEST, "Séquence vide — rien à jouer").into_response();
    }
    let (notes_arrays, beats, rcfg) = render_inputs(&b);
    let (smf, total_beats, all_notes) = if !b.custom_notes.is_empty() || !b.custom_channels.is_empty() {
        let custom_channels: std::collections::HashSet<u8> = b.custom_channels
            .iter()
            .copied()
            .chain(b.custom_notes.iter().map(|n| n.channel))
            .collect();
        let classic = render::generate_notes(&notes_arrays, &beats, &rcfg);
        let mut merged: Vec<render::CustomNote> = classic
            .into_iter()
            .filter(|n| !custom_channels.contains(&n.channel))
            .collect();
        for cn in &b.custom_notes {
            let t = rcfg.tracks.iter().find(|t| t.channel == cn.channel);
            // Piste mutée ou absente → silencieuse (logique métier : mute partout)
            if t.map_or(true, |t| t.mute) {
                continue;
            }
            // Piste percussion hors canal 9 → redirigée vers le 9 (le kit)
            let is_drum = t.map_or(false, |t| t.drums);
            let out_ch = if is_drum && cn.channel != 9 { 9 } else { cn.channel };
            let vol = t.map_or(127, |t| t.volume) as u32;
            let v = ((cn.velocity as u32 * vol) / 127).clamp(0, 127) as u8;
            merged.push(render::CustomNote {
                channel: out_ch,
                start_time: cn.start_time,
                pitch: cn.pitch,
                duration: cn.duration,
                velocity: v,
            });
        }
        let tracks: Vec<render::TrackCfg> = rcfg.tracks.to_vec();
        let smf = render::generate_smf_from_custom(&merged, &tracks, b.tempo as u16);
        let tb = merged.iter().map(|n| n.start_time + n.duration).fold(0.0, f64::max);
        (smf, tb, merged)
    } else {
        let smf = render::generate_smf_fmt0(&notes_arrays, &beats, &rcfg);
        let tb = beats.iter().sum();
        let classic = render::generate_notes(&notes_arrays, &beats, &rcfg);
        (smf, tb, classic)
    };

    let sf_path = s.soundfont.as_deref().unwrap_or("/usr/share/sounds/sf3/MuseScore_General_Full.sf3");
    let duration_sec = total_beats * 60.0 / b.tempo.max(1) as f64;
    let has_fx = rcfg.tracks.iter().any(|t| !t.fx.is_off());
    let main_wav = if has_fx {
        render::render_wav_mixed(&all_notes, &rcfg, sf_path, duration_sec, b.master_vol)
    } else {
        render::render_wav(&smf, sf_path, duration_sec, b.master_vol)
    };
    let main_wav = match main_wav {
        Ok(w) => w,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("Rendu : {}", e)).into_response(),
    };

    // 2. Sortie dédiée (l'appareil MULTICANAL — agrégat)
    let cfg = click::load(&s.click);
    let out_device = match cfg.out_device {
        Some(d) if !d.is_empty() => d,
        _ => {
            return (StatusCode::BAD_REQUEST,
                "Aucune sortie dédiée configurée — choisis l'appareil multicanal (agrégat) dans le contrôle Clic")
                .into_response()
        }
    };

    // 3. Rendu du clic
    let bars = (sig_code(&b.sig) / 10).max(1) as u64;
    let click_smf = render::generate_click_smf(b.tempo.max(1) as u16, bars, total_beats, cfg.accent, cfg.sound);
    let click_wav = match render::render_wav(&click_smf, sf_path, duration_sec, 127) {
        Ok(c) => c,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("Rendu clic : {}", e)).into_response()
        }
    };

    // 4. Fichiers temporaires + lecture : double canaux si l'appareil est
    //    MULTICANAL (≥ 4 canaux — agrégat Mac / multi ALSA), sinon REPLI
    //    automatique : clic MÉLANGÉ au son principal (synchro parfaite
    //    quand même — un seul WAV). Plus jamais d'erreur bloquante.
    //    start_sec : les deux WAV démarrent à cet offset (alignés), le
    //    repli mixé est tronqué au même offset.
    let dir = std::env::temp_dir().join("chordj_rendered");
    let _ = std::fs::create_dir_all(&dir);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let main_name = format!("dual_{}_main.wav", ts);
    let click_name = format!("dual_{}_click.wav", ts);
    let _ = std::fs::write(dir.join(&main_name), &main_wav);
    let _ = std::fs::write(dir.join(&click_name), &click_wav);
    // Mettre en cache ce rendu : les prochains scrubs (start_at) le
    // réutiliseront sans re-rendre (relance instantanée, comme le MIDI).
    {
        let mp = dir.join(&main_name).to_str().unwrap_or("").to_string();
        let cp = dir.join(&click_name).to_str().unwrap_or("").to_string();
        *s.rendered_dual.lock().unwrap() = Some((mp, cp, duration_sec));
    }
    let gain = (cfg.volume as f32 / 100.0) * 1.0;
    let main_path = dir.join(&main_name).to_str().unwrap_or("").to_string();
    let click_path = dir.join(&click_name).to_str().unwrap_or("").to_string();

    // Essai du mode SÉPARÉ (appareil 4 canaux : agrégat Mac / multi ALSA).
    // En cas d'échec (sortie non multicanal, introuvable, occupée…) →
    // REPLI automatique : clic MÉLANGÉ au son principal (synchro parfaite).
    // Le décalage/volume du clic sont lus EN DIRECT dans l'état partagé
    // (réglables pendant la lecture).
    match click::play_dual(&main_path, &click_path, &out_device, s.click.clone(), start_sec, loop_playback, loop_start_sec, loop_end_sec) {
        Ok(()) => (
            StatusCode::OK,
            axum::Json(serde_json::json!({
                "ok": true,
                "mode": "channels",
                "duration_sec": (duration_sec - start_sec).max(0.0),
                "total_duration_sec": duration_sec,
            })),
        ).into_response(),
        Err(e) => {
            // Repli : clic mélangé (1 seul WAV, synchro parfaite)
            let mixed = match render::mix_wavs(&main_wav, &click_wav, gain) {
                Ok(m) => m,
                Err(me) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, format!("Mix : {}", me)).into_response()
                }
            };
            let mixed = render::slice_wav_from(&mixed, start_sec);
            let mixed_name = format!("dual_{}_mixed.wav", ts);
            let _ = std::fs::write(dir.join(&mixed_name), &mixed);
            eprintln!("   ℹ️ Clic : repli MIXÉ (sortie « {} » : {})", out_device, e);
            match click::play_click_wav(
                dir.join(&mixed_name).to_str().unwrap_or(""),
                Some(out_device.clone()),
                0,
                0,
            ) {
                Ok(()) => (
                    StatusCode::OK,
                    axum::Json(serde_json::json!({
                        "ok": true,
                        "mode": "mixed_fallback",
                        "reason": format!("Sortie « {} » non multicanal → clic mélangé au son principal (synchro parfaite)", out_device),
                        "duration_sec": (duration_sec - start_sec).max(0.0),
                        "total_duration_sec": duration_sec,
                    })),
                ).into_response(),
                Err(e2) => (StatusCode::BAD_REQUEST, e2).into_response(),
            }
        }
    }
}

// ─── Main ───────────────────────────────────────────────────────────────

/// Liste des samples disponibles (boucle sample du mode Navig).
async fn samples_list() -> impl IntoResponse {
    let data = samples::get_available();
    (StatusCode::OK, axum::Json(data))
}

/// Sert un fichier sample (~/samples/drums/<name>) au navigateur — utilisé
/// par la boucle sample du mode Navig (lecture Web Audio côté client).
async fn sample_file(axum::extract::Path(name): axum::extract::Path<String>) -> impl IntoResponse {
    // Sécurité : nom de fichier simple uniquement (pas de chemin)
    if name.contains('/') || name.contains("\\") || name.contains("..") {
        return (StatusCode::BAD_REQUEST, "nom de fichier invalide").into_response();
    }
    let dir = samples::drum_dir();
    match std::fs::read(std::path::Path::new(&dir).join(&name)) {
        Ok(data) => {
            let typ = if name.to_ascii_lowercase().ends_with(".wav") {
                "audio/wav"
            } else {
                "application/octet-stream"
            };
            (StatusCode::OK, [(header::CONTENT_TYPE, typ)], data).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "sample introuvable").into_response(),
    }
}

/// Construit le routeur Axum complet (routes API + fallback frontend).
/// Séparé de `main` pour être testable : les tests d'intégration HTTP
/// montent l'app en mémoire (tower::ServiceExt::oneshot) sans socket.
///
/// En mode standalone, les routes du frontend sont servies par
/// `frontend_embed::serve`. En mode dev, la route GET / sert l'index.html
/// statique (le frontend est servi par Vite sur :5176).
fn build_app(state: AppState) -> Router {
    let mut app = Router::new()
        .route("/play", post(play))
        .route("/config", post(conf))
        .route("/stop", post(stop))
        .route("/render-wav", post(render_wav))
        .route("/render-tracks", post(render_tracks))
        .route("/render-notes", post(render_notes))
        .route("/note", post(note))
        .route("/samples-list", get(samples_list))
        .route("/sample-file/:name", get(sample_file))
        .route("/rendered/:file", get(serve_rendered))
        .route("/audio-devices", get(audio_devices))
        .route("/audio-device", post(audio_device))
        .route("/midi-ports", get(midi_ports))
        .route("/midi-port", post(midi_port))
        .route("/click", get(get_click).post(post_click))
        .route("/navig-click-start", post(navig_click_start))
        .route("/navig-click-stop", post(navig_click_stop))
        .route("/navig-play", post(navig_play))
        .route("/navig-play-midi", post(navig_play_midi))
        .route("/navig-stop-midi", post(navig_stop_midi))
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
    }

    #[cfg(not(feature = "standalone"))]
    {
        // Mode dev : servir l'ancien index.html statique sur /
        app = app.route("/", get(idx));
    }
    app
}

#[tokio::main]
async fn main() {
    println!("🚀 chordZIC backend — serveur de séquencement MIDI");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    // Détection automatique de la SoundFont
    let soundfont = find_soundfont();

    // Initialisation MIDI
    let midi = Arc::new(Mutex::new(init_midi().map(|(handle, port)| MidiLink { handle, port })));

    use patterns::PAT_ROCK;
    use std::sync::atomic::{AtomicU16, AtomicU8};
    use std::sync::Mutex;

    let state = AppState {
        midi,
        midi_gen: Arc::new(std::sync::atomic::AtomicU64::new(0)),
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
        }),
        click: Arc::new(click::ClickState {
            volume: std::sync::atomic::AtomicU8::new(
                std::env::var("CLICK_VOLUME").ok().and_then(|v| v.parse().ok()).unwrap_or(80),
            ),
            accent: std::sync::atomic::AtomicBool::new(
                std::env::var("CLICK_ACCENT").map(|v| v != "0").unwrap_or(true),
            ),
            sound: std::sync::atomic::AtomicU8::new(
                std::env::var("CLICK_SOUND").ok().and_then(|v| v.parse().ok()).unwrap_or(0),
            ),
            in_render: std::sync::atomic::AtomicBool::new(
                std::env::var("CLICK_IN_RENDER").map(|v| v == "1").unwrap_or(false),
            ),
            out_device: std::sync::Mutex::new(
                std::env::var("CLICK_OUT_DEVICE").ok().filter(|s| !s.is_empty()),
            ),
            delay_ms: std::sync::atomic::AtomicI32::new(
                std::env::var("CLICK_DELAY_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0),
            ),
        }),
        rendered_dual: Arc::new(Mutex::new(None)),
    };

    let port = std::env::var("PORT").unwrap_or_else(|_| "4000".to_string());

    let app = build_app(state);


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
    println!("     POST /render-tracks → bounce multitrack (mode PostProd)");
    println!("     GET  /rendered/<f>  → WAV temporaire du bounce PostProd");
    println!("     POST /render-notes  → notes mode classique (PianoRoll)");
    println!("     POST /note          → audition note en direct (preview)");
    println!("     GET  /audio-devices → lister les sorties audio (cpal)");
    println!("     GET  /click         → config de la piste de clic");
    println!("     POST /click         → modifier la config du clic");
    println!("     GET  /samples-list  → boucles WAV disponibles");
    println!("     POST /save          → sauvegarder une grille (JSON)");
    println!("     GET  /grilles       → lister les grilles");
    println!("     DELETE /grilles/<n> → supprimer une grille\n");

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simule l'horloge du clic métronome MIDI (nouvelle version) : la cible
    /// est recalculée à chaque beat (b × tempo + décalage) → intervalles
    /// stables, le décalage décale le 1ᵉʳ clic sans casser le tempo.
    #[test]
    fn clic_metronome_intervalles_stables() {
        let tempo_ms = 500.0; // 120 BPM
        let note_dur = 150.0; // durée de note inline (ms)
        let start_at = 0.0f64;
        let shift_ms = 30.0; // décalage du clic
        let mut b = start_at.ceil() as u64;
        let mut elapsed = 0.0f64;
        let mut onsets = Vec::new();
        for _ in 0..10 {
            let target = (((b as f64 - start_at) * tempo_ms) + shift_ms).max(0.0);
            elapsed += (target - elapsed).max(0.0);
            onsets.push(elapsed); // note ON
            elapsed += note_dur; // note OFF + overhead
            b += 1;
        }
        // 1ᵉʳ clic décalé de shift_ms ; ensuite intervalles = tempo_ms
        assert!((onsets[0] - shift_ms).abs() < 1e-9, "1ᵉʳ clic non décalé");
        for w in onsets.windows(2) {
            let iv = w[1] - w[0];
            assert!((iv - tempo_ms).abs() < 1e-9, "intervalle {iv} != {tempo_ms}");
        }
    }

    /// Conversion start_at (beats) → secondes pour navig-play (offset de
    /// lecture du rendu WAV, main + clic alignés).
    #[test]
    fn start_sec_from_beats_conversion() {
        assert_eq!(start_sec_from_beats(None, 120), 0.0);
        assert_eq!(start_sec_from_beats(Some(0.0), 120), 0.0);
        assert_eq!(start_sec_from_beats(Some(4.0), 120), 2.0);
        assert_eq!(start_sec_from_beats(Some(1.0), 60), 1.0);
        assert!((start_sec_from_beats(Some(2.5), 123) - 1.219512195).abs() < 1e-6);
    }

    /// Boucle MIDI : fenêtre temporelle (cible du note-on, fin de passe pour
    /// le note-off) d'une note à la passe `pass` — passe 0 = depuis start_at
    /// (durée = pass0_len), passes suivantes = morceau complet (loop_len).
    #[test]
    fn note_loop_window_repetitions() {
        let tempo_ms = 500.0; // 120 BPM
        // Morceau de 8 temps, locators par défaut [0, 8[
        let (loop_start, loop_end) = (0.0, 8.0);
        // Passe 0 avec start_at : décalage depuis start_at, fin = pass0_len
        let (t, e) = note_loop_window(0, 2.0, 1.0, loop_start, loop_end, tempo_ms);
        assert!((t - 500.0).abs() < 1e-9);
        assert!((e - 3500.0).abs() < 1e-9); // 8 temps − 1 temps de start_at
        // Passe 0 sans start_at : la note à son temps, fin = morceau complet
        let (t, e) = note_loop_window(0, 2.0, 0.0, loop_start, loop_end, tempo_ms);
        assert!((t - 1000.0).abs() < 1e-9);
        assert!((e - 4000.0).abs() < 1e-9);
        // Passe 1 (sans start_at) : un tour complet + le temps de la note
        let (t, e) = note_loop_window(1, 2.0, 0.0, loop_start, loop_end, tempo_ms);
        assert!((t - (4000.0 + 1000.0)).abs() < 1e-9);
        assert!((e - 8000.0).abs() < 1e-9);
        // Passe 2 : deux tours + le temps de la note
        let (t, _) = note_loop_window(2, 2.0, 0.0, loop_start, loop_end, tempo_ms);
        assert!((t - (8000.0 + 1000.0)).abs() < 1e-9);
        // Note avant start_at → passe 0 sautée ; la passe 1 commence à la
        // FIN de la passe 0 (3000 ms), pas à un tour complet (ancien bug :
        // 4250 → la boucle attendait la fin du morceau entier).
        let (t, e) = note_loop_window(1, 0.5, 2.0, loop_start, loop_end, tempo_ms);
        assert!((t - (3000.0 + 250.0)).abs() < 1e-9);
        assert!((e - 7000.0).abs() < 1e-9);
    }

    /// Scrub + boucle : après un déplacement de tête, la passe 0 est plus
    /// courte que le morceau complet, les cycles suivants durent loop_len —
    /// le clic et les notes partagent ce référentiel (synchro au repeat).
    #[test]
    fn note_loop_window_scrub_puis_boucle() {
        let tempo_ms = 500.0;
        let (loop_start, loop_end) = (0.0, 32.0); // 32 temps
        // Passe 0 depuis le beat 24 : note à 24 → immédiate, fin de passe 4000
        let (t, e) = note_loop_window(0, 24.0, 24.0, loop_start, loop_end, tempo_ms);
        assert!((t - 0.0).abs() < 1e-9);
        assert!((e - 4000.0).abs() < 1e-9);
        // Sa passe 1 : 4000 (fin de passe 0) + 24×500 = 16000, fin 20000
        let (t, e) = note_loop_window(1, 24.0, 24.0, loop_start, loop_end, tempo_ms);
        assert!((t - 16000.0).abs() < 1e-9);
        assert!((e - 20000.0).abs() < 1e-9);
        // Note du début (2) : jamais en passe 0, passe 1 = 4000 + 1000
        let (t, _) = note_loop_window(1, 2.0, 24.0, loop_start, loop_end, tempo_ms);
        assert!((t - 5000.0).abs() < 1e-9);
    }

    /// Locators [L, R[ : la boucle couvre l'intervalle au lieu du morceau
    /// complet — passe 0 depuis start_at jusqu'à R, puis cycles [L, R[.
    #[test]
    fn note_loop_window_intervalle_locators() {
        let tempo_ms = 500.0;
        // Locators [8, 16[, scrub à 12 : passe 0 = 12..16 (2000 ms), puis
        // cycles [8, 16[ (4000 ms).
        let (t, e) = note_loop_window(0, 12.0, 12.0, 8.0, 16.0, tempo_ms);
        assert!((t - 0.0).abs() < 1e-9);
        assert!((e - 2000.0).abs() < 1e-9);
        // Passe 1 : 2000 (fin passe 0) + (12−8)×500 = 4000 ; fin 2000+4000
        let (t, e) = note_loop_window(1, 12.0, 12.0, 8.0, 16.0, tempo_ms);
        assert!((t - 4000.0).abs() < 1e-9);
        assert!((e - 6000.0).abs() < 1e-9);
        // Passe 2 : 2000 + 4000 + (12−8)×500 = 8000
        let (t, _) = note_loop_window(2, 12.0, 12.0, 8.0, 16.0, tempo_ms);
        assert!((t - 8000.0).abs() < 1e-9);
        // Note du début de l'intervalle (9) : jamais en passe 0 (avant
        // start_at 12), passe 1 = 2000 + (9−8)×500 = 2500
        let (t, _) = note_loop_window(1, 9.0, 12.0, 8.0, 16.0, tempo_ms);
        assert!((t - 2500.0).abs() < 1e-9);
    }

    // ── Scheduler mono-thread : sélection des notes (features locators) ──

    fn note(start_time: f64, duration: f64) -> render::CustomNote {
        render::CustomNote {
            channel: 0, start_time, pitch: 60, duration, velocity: 100,
        }
    }

    /// select_notes : les notes ≥ locator droit ne sont JAMAIS jouées,
    /// le tout est trié par start_time.
    #[test]
    fn select_notes_exclut_les_notes_apres_le_locator_droit() {
        let notes = vec![note(4.0, 1.0), note(20.0, 1.0), note(15.0, 1.0), note(3.0, 1.0)];
        let sel = select_notes(notes, true, 0.0, 0.0, 16.0);
        let starts: Vec<f64> = sel.iter().map(|n| n.start_time).collect();
        assert_eq!(starts, vec![3.0, 4.0, 15.0]); // 20 ≥ 16 → exclue, tri ✓
    }

    /// select_notes sans boucle : les notes finissant avant start_at (scrub)
    /// sont ignorées ; en boucle, TOUT l'intervalle est gardé (les passes
    /// suivantes repartent de L).
    #[test]
    fn select_notes_scrub_et_boucle() {
        // Sans boucle : note finissant à 10, scrub à 12 → ignorée
        let notes = vec![note(8.0, 2.0), note(12.0, 1.0)];
        let sel = select_notes(notes.clone(), false, 12.0, 0.0, 32.0);
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].start_time, 12.0);
        // Avec boucle : même note gardée (elle rejouera aux cycles suivants)
        let sel = select_notes(notes, true, 12.0, 0.0, 32.0);
        assert_eq!(sel.len(), 2);
    }

    /// select_notes : une note avant le locator gauche reste dans la liste
    /// (elle est jouée une seule fois en passe 0 si elle déborde sur L).
    #[test]
    fn select_notes_garde_les_notes_avant_le_locator_gauche() {
        let notes = vec![note(6.0, 3.0)]; // déborde jusqu'à 9 (L = 8)
        let sel = select_notes(notes, true, 0.0, 8.0, 16.0);
        assert_eq!(sel.len(), 1, "note avant L jouée une fois en passe 0");
    }

    // ── Scheduler mono-thread : clamp du note-off ──

    /// note_off_clamped : en boucle, le note-off est clampé à la fin de passe
    /// (−2 ms) pour qu'il passe AVANT le note-on du cycle suivant ; sans
    /// boucle, la durée brute est conservée.
    #[test]
    fn note_off_clamped_boucle_et_sans_boucle() {
        // Boucle : note finissant après la fin de passe → coupée à fin − 2 ms
        let d = note_off_clamped(1000.0, 2000.0, 2000.0, true);
        assert!((d - 998.0).abs() < 1e-9);
        // Note courte dans la passe → durée brute conservée
        let d = note_off_clamped(1000.0, 2000.0, 500.0, true);
        assert!((d - 500.0).abs() < 1e-9);
        // Jamais négatif (note qui commencerait après la fin de passe)
        let d = note_off_clamped(1999.0, 2000.0, 500.0, true);
        assert_eq!(d, 0.0);
        // Sans boucle : durée brute, même si elle dépasse la passe
        let d = note_off_clamped(1000.0, 2000.0, 2000.0, false);
        assert!((d - 2000.0).abs() < 1e-9);
    }

    // ── Scheduler mono-thread : wrap du clic sur [L, R[ ──

    /// next_click_state : le clic reboucle à L quand il atteint R, l'offset
    /// avance d'un cycle (fin de passe 0 + (cycle−1) × durée de cycle).
    #[test]
    fn next_click_state_wrap_locators() {
        // Locators [8, 16[, scrub à 12 : passe 0 = 2000 ms, cycles = 4000 ms
        let (b, c, o) = next_click_state(15, 0, 0.0, true, 8.0, 16.0, 2000.0, 4000.0, 32);
        assert_eq!((b, c), (15, 0)); // 15 < 16 : pas de wrap
        assert_eq!(o, 0.0);
        let (b, c, o) = next_click_state(16, 0, 0.0, true, 8.0, 16.0, 2000.0, 4000.0, 32);
        assert_eq!((b, c), (8, 1)); // wrap à L, 1er cycle
        assert!((o - 2000.0).abs() < 1e-9);
        // 2e wrap : offset = 2000 + 4000
        let (b, c, o) = next_click_state(16, 1, 2000.0, true, 8.0, 16.0, 2000.0, 4000.0, 32);
        assert_eq!((b, c), (8, 2));
        assert!((o - 6000.0).abs() < 1e-9);
        // Début de lecture sans scrub : passe 0 = intervalle complet (0 ms)
        let (b, c, o) = next_click_state(16, 0, 0.0, true, 8.0, 16.0, 8000.0, 4000.0, 32);
        assert_eq!((b, c), (8, 1));
        assert!((o - 8000.0).abs() < 1e-9);
    }

    /// next_click_state sans boucle : après la fin du morceau, plus aucun
    /// clic (état inchangé) ; le beat loop_end n'est jamais atteint en wrap.
    #[test]
    fn next_click_state_fin_de_lecture_sans_boucle() {
        let (b, c, o) = next_click_state(33, 0, 0.0, false, 0.0, 32.0, 0.0, 16000.0, 32);
        assert_eq!((b, c), (33, 0)); // inchangé → l'appelant ne planifie rien
        assert_eq!(o, 0.0);
        // Dernier beat valide : clic planifié normalement
        let (b, _, _) = next_click_state(32, 0, 0.0, false, 0.0, 32.0, 0.0, 16000.0, 32);
        assert_eq!(b, 32);
    }

    // ── Tests d'intégration HTTP (app montée en mémoire, sans socket) ──
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    /// État de test : pas de connexion MIDI, pas de SoundFont, config par
    /// défaut — les routes de validation pure sont testables sans matériel.
    fn test_state() -> AppState {
        use std::sync::atomic::{AtomicU16, AtomicU8};
        AppState {
            midi: Arc::new(Mutex::new(None)),
            midi_gen: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            soundfont: None,
            live: Arc::new(Live {
                tracks: Mutex::new(vec![
                    LiveTrack::new(0, 51, 60),
                    LiveTrack::new(2, 33, 70),
                    LiveTrack::new(3, 48, 60),
                    LiveTrack::new(9, 1, 90),
                    LiveTrack::new(4, 2, 50),
                ]),
                pattern: AtomicU8::new(patterns::PAT_ROCK),
                tempo: AtomicU16::new(120),
                stop: AtomicBool::new(false),
                sig: AtomicU16::new(44),
                walking: AtomicBool::new(false),
                master_vol: AtomicU8::new(127),
                use432: AtomicBool::new(false),
            }),
            click: Arc::new(click::ClickState::default()),
            rendered_dual: Arc::new(Mutex::new(None)),
        }
    }

    /// Envoie une requête HTTP à l'app montée en mémoire.
    async fn req(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, String) {
        let mut rb = axum::http::Request::builder().method(method).uri(uri);
        if body.is_some() {
            rb = rb.header("content-type", "application/json");
        }
        let b = axum::body::Body::from(body.map(|v| v.to_string()).unwrap_or_default());
        let resp = app.clone().oneshot(rb.body(b).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    /// Les routes de lecture refusent une séquence vide (400) AVANT tout
    /// accès MIDI — testable sans matériel.
    #[tokio::test]
    async fn api_sequence_vide_refusee_sur_les_routes_de_lecture() {
        let app = build_app(test_state());
        for uri in ["/render-wav", "/navig-play", "/navig-play-midi"] {
            let (s, body) = req(&app, "POST", uri, Some(serde_json::json!({}))).await;
            assert_eq!(
                s,
                StatusCode::BAD_REQUEST,
                "{uri} doit refuser la séquence vide (reçu {s}: {body})"
            );
        }
    }

    /// GET /click renvoie la config complète du clic (JSON).
    #[tokio::test]
    async fn api_click_retourne_la_config() {
        let app = build_app(test_state());
        let (s, body) = req(&app, "GET", "/click", None).await;
        assert_eq!(s, StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        for key in ["volume", "accent", "sound", "in_render", "delay_ms", "sounds"] {
            assert!(v.get(key).is_some(), "GET /click doit contenir {key}");
        }
    }

    /// GET /sample-file : nom avec chemin → 400 (sécurité), inconnu → 404.
    #[tokio::test]
    async fn api_sample_file_valide_les_noms() {
        let app = build_app(test_state());
        let (s, _) = req(&app, "GET", "/sample-file/..%2F..", None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "path traversal refusé");
        let (s, _) = req(&app, "GET", "/sample-file/inexistant_xyz.wav", None).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "sample inconnu → 404");
    }

    /// POST /render-wav avec une séquence valide : rendu WAV réel (via
    /// FluidSynth) → 200 + body audio/wav non vide. C'est le test
    /// d'intégration le plus proche du flux réel (grille → SMF → WAV).
    #[tokio::test]
    async fn api_render_wav_sequence_valide_rend_un_wav() {
        let app = build_app(test_state());
        let body = serde_json::json!({
            "sequence": [{"notes": ["C4", "E4", "G4"], "beats": 4.0}],
            "tempo": 120,
            "pattern": "rock",
            "walking": false,
            "sig": "4/4",
            "master_vol": 127
        });
        let (s, body) = req(&app, "POST", "/render-wav", Some(body)).await;
        assert_eq!(s, StatusCode::OK, "render-wav doit réussir : {body}");
        // Réponse = WAV brut (audio/wav) : commence par le header RIFF
        assert!(body.starts_with("RIFF"), "réponse WAV attendue, reçu: {}", &body[..body.len().min(80)]);
    }

    /// POST /save + GET /grilles + DELETE : cycle de vie d'une grille
    /// (dossier isolé via HOME temporaire — aucun impact sur les vraies
    /// grilles de l'utilisateur).
    #[tokio::test]
    async fn api_grilles_cycle_de_vie() {
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());
        let result = async {
            let app = build_app(test_state());
            // Sauvegarde
            let (s, body) = req(
                &app,
                "POST",
                "/save",
                Some(serde_json::json!({"name": "test_integration", "grille": [1, 2, 3]})),
            ).await;
            assert_eq!(s, StatusCode::OK, "save doit réussir : {body}");
            // Liste — la grille doit apparaître
            let (s, body) = req(&app, "GET", "/grilles", None).await;
            assert_eq!(s, StatusCode::OK);
            let v: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert!(
                body.contains("test_integration"),
                "la grille doit apparaître dans la liste : {body}"
            );
            // Suppression
            let (s, _) = req(&app, "DELETE", "/grilles/test_integration", None).await;
            assert_eq!(s, StatusCode::OK, "delete doit réussir");
            let (_, body) = req(&app, "GET", "/grilles", None).await;
            assert!(!body.contains("test_integration"), "grille supprimée : {body}");
            v
        }.await;
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        let _ = result; // évite un warning si le contenu n'est pas utilisé
    }
}
