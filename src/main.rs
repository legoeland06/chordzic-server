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
mod midi;
mod patterns;
mod render;
mod samples;
mod walking;

use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use patterns::pat;
use midi::{
    apply_tracks, init_midi, note_midi, play_seq, play_notes, rch, pb, ChordEv, Live, LiveTrack,
    MidiHandle, TrackCfg as MidiTrackCfg, TRACK_BASS, TRACK_DRUMS, TRACK_LEAD, TRACK_STR,
};

// ─── État global ────────────────────────────────────────────────────────

/// État partagé du serveur, injecté dans chaque route via Axum State.
#[derive(Clone)]
struct AppState {
    midi: Option<MidiHandle>,   // Connexion MIDI vers FluidSynth (None si pas de port)
    live: Arc<Live>,            // État live mutable partagé entre threads HTTP et audio
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

/// GET / — Page d'accueil (inclut le HTML statique embed).
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
    // Modifie l'état live AVANT de lancer le thread pour éviter
    // les conditions de course.
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
        lv.tracks[TRACK_LEAD].program.store(b.inst_val, std::sync::atomic::Ordering::Relaxed);
        lv.tracks[TRACK_LEAD].mute.store(!b.arps, std::sync::atomic::Ordering::Relaxed);
        lv.tracks[TRACK_BASS].mute.store(!b.bass, std::sync::atomic::Ordering::Relaxed);
        lv.tracks[TRACK_STR].mute.store(!b.nappes, std::sync::atomic::Ordering::Relaxed);
        lv.tracks[TRACK_DRUMS].mute.store(!b.drums, std::sync::atomic::Ordering::Relaxed);
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
    if let Some(ref h) = s.midi {
        let h2 = Arc::clone(h);

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
            // Thread séparé pour la séquence : ne pas bloquer la réponse HTTP
            let sq = ev.to_vec();
            let l = Arc::clone(lv);
            std::thread::spawn(move || {
                if let Ok(mut c) = h2.lock() {
                    play_seq(&mut c, &sq, &l, do_loop);
                }
            });
        } else if let Some(ref n) = b.notes {
            // Notes immédiates (pas de séquence) — utile pour les tests
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
///
/// Contrairement à `/play`, cette route ne lance pas de nouvelle séquence.
/// Elle met à jour l'état live atomiquement pendant que le thread audio
/// continue de jouer.
async fn conf(State(s): State<AppState>, Json(b): Json<Cfg>) -> impl IntoResponse {
    let lv = &s.live;

    // Mise à jour des pistes (si fournie)
    if let Some(ref t) = b.tracks {
        apply_tracks(lv, t);
    }

    // Mise à jour des flags individuels (chaque Option est testée)
    if let Some(v) = b.drums {
        lv.tracks[TRACK_DRUMS].mute.store(!v, std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(v) = b.bass {
        lv.tracks[TRACK_BASS].mute.store(!v, std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(v) = b.arpeggios {
        lv.tracks[TRACK_LEAD].mute.store(!v, std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(v) = b.nappes {
        lv.tracks[TRACK_STR].mute.store(!v, std::sync::atomic::Ordering::Relaxed);
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
        lv.tracks[TRACK_LEAD].program.store(iv, std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(w) = b.walking {
        lv.walking.store(w, std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(m) = b.master_vol {
        lv.master_vol.store(m, std::sync::atomic::Ordering::Relaxed);
    }

    // Accordage 432Hz — envoie un pitch bend sur tous les canaux musicaux
    if let Some(u) = b.use432 {
        let was = lv.use432.swap(u, std::sync::atomic::Ordering::Relaxed);
        if was != u {
            // Appliquer le pitch bend immédiatement sur les canaux 0,2,3,4
            // Valeur 6881 = pitch down de ~32 centièmes (pour passer de 440 à 432 Hz)
            // 8192 = pas de bend (440 Hz)
            if let Some(ref h) = s.midi {
                if let Ok(mut c) = h.lock() {
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
///
/// 1. Positionne le flag `stop` à true (le thread audio le détecte au
///    prochain beat et sort de la boucle).
/// 2. Arrête la boucle WAV drums.
/// 3. Envoie All Notes Off (CC 123) sur tous les canaux.
async fn stop(State(s): State<AppState>) -> impl IntoResponse {
    s.live.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    samples::stop_loop();
    if let Some(ref h) = s.midi {
        if let Ok(mut c) = h.lock() {
            rch(&mut c); // All Notes Off sur tous les canaux
        }
    }
    Json(serde_json::json!({"status": "stopped"}))
}

/// POST /render-wav — Rendu batch d'une séquence en WAV.
///
/// Contrairement à `/play` (live MIDI temps réel), cette route :
/// - Génère un fichier SMF Format 0 complet en mémoire
/// - Appelle FluidSynth pour le convertir en WAV
/// - Retourne le fichier WAV directement dans la réponse HTTP
///
/// C'est une opération synchrone : la réponse contient le fichier WAV
/// complet, prêt à être téléchargé par le navigateur.
async fn render_wav(Json(b): Json<PlayReq>) -> impl IntoResponse {
    use axum::http::HeaderMap;

    // Extraire la séquence
    let ev: &[ChordEv] = if !b.seq.is_empty() {
        &b.seq
    } else if !b.sequence.is_empty() {
        &b.sequence
    } else {
        &[]
    };

    if ev.is_empty() {
        return (StatusCode::BAD_REQUEST, "Séquence vide — rien à rendre").into_response();
    }

    // Convertir les ChordEv en tableaux de notes MIDI + durées
    let mut notes_arrays: Vec<Vec<u8>> = Vec::new();
    let mut beats: Vec<f64> = Vec::new();
    for e in ev {
        notes_arrays.push(notes_from_ev(e));
        beats.push(e.beats);
    }

    // Configuration des tracks pour le render
    // (valeurs par défaut, surchargées par `tracks` si fourni)
    let mut tracks_cfg: [render::TrackCfg; 5] = [
        render::TrackCfg { channel: 0, program: b.inst_val, volume: 15, mute: !b.arps },
        render::TrackCfg { channel: 2, program: 33, volume: 40, mute: !b.bass },
        render::TrackCfg { channel: 3, program: 48, volume: 30, mute: !b.nappes },
        render::TrackCfg { channel: 9, program: 1, volume: 80, mute: !b.drums },
        render::TrackCfg { channel: 4, program: 2, volume: 20, mute: false },
    ];

    // Fusion avec la configuration des pistes envoyée par le frontend
    if let Some(ref tcfg) = b.tracks {
        for tc in tcfg {
            if let Some(t) = tracks_cfg.iter_mut().find(|t| t.channel == tc.channel) {
                t.program = tc.program.unwrap_or(t.program);
                t.volume = tc.volume.unwrap_or(t.volume);
                t.mute = tc.mute.unwrap_or(t.mute);
            }
        }
    }

    // Créer la configuration de rendu
    let rcfg = render::RenderCfg {
        tempo: b.tempo,
        pattern: b.pattern.clone(),
        walking: b.walking.unwrap_or(false),
        sig: b.sig.clone(),
        lead_inst: b.inst_val,
        tracks: tracks_cfg,
    };

    // Générer le SMF
    let smf = render::generate_smf_fmt0(&notes_arrays, &beats, &rcfg);

    // Chemin de la SoundFont (MuseScore General)
    let sf_path = "/usr/share/sounds/sf3/MuseScore_General_Full.sf3";

    // Durée totale en secondes (pour le trim)
    let total_beats: f64 = beats.iter().sum();
    let duration_sec = total_beats * 60.0 / b.tempo.max(1) as f64;

    // Lancer le rendu FluidSynth
    match render::render_wav(&smf, sf_path, duration_sec) {
        Ok(wav) => {
            // Retourner le WAV avec le bon Content-Type
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

/// Point d'entrée du serveur.
///
/// Initialisation :
/// 1. Connexion MIDI (vers FluidSynth ou Roland)
/// 2. Scan des boucles WAV drums
/// 3. Création de l'état global AppState
/// 4. Configuration des routes
/// 5. Démarrage du serveur HTTP sur le port 4000 (ou variable PORT)
#[tokio::main]
async fn main() {
    println!("🚀 chordZIC backend — serveur de séquencement MIDI");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    // Initialisation
    let midi = init_midi();            // Connexion MIDI (peut être None)
    samples::init();                   // Scan des boucles WAV drums

    use patterns::PAT_ROCK;
    use std::sync::atomic::{AtomicI32, AtomicU16, AtomicU8};
    use std::sync::Mutex;

    // Création de l'état global (5 pistes MIDI)
    let state = AppState {
        midi,
        live: Arc::new(Live {
            tracks: [
                LiveTrack::new(0, 51, 15),   // Lead : Synth Strings 1 (canal 0)
                LiveTrack::new(2, 33, 40),   // Bass : Electric Bass (finger) (canal 2)
                LiveTrack::new(3, 48, 30),   // Strings : String Ensemble 1 (canal 3)
                LiveTrack::new(9, 1, 80),    // Drums : Standard Kit (canal 9)
                LiveTrack::new(4, 2, 20),    // Accent : Bright Acoustic Piano (canal 4)
            ],
            pattern: AtomicU8::new(PAT_ROCK),
            tempo: AtomicU16::new(120),
            stop: AtomicBool::new(false),
            sig: AtomicU16::new(44),          // 4/4
            walking: AtomicBool::new(false),
            master_vol: AtomicU8::new(127),   // Master volume max
            use432: AtomicBool::new(false),   // 440Hz par défaut
            loop_offset: AtomicI32::new(0),
            use_loops: AtomicBool::new(false),
            loop_name: Mutex::new(String::new()),
            loop_volume: AtomicU8::new(80),
        }),
    };

    /// GET /samples-list — Liste les boucles WAV disponibles.
    async fn samples_list() -> impl IntoResponse {
        let data = samples::get_available();
        (StatusCode::OK, axum::Json(data))
    }

    // Configuration des routes HTTP et démarrage
    let app = Router::new()
        .route("/", get(idx))
        .route("/play", post(play))
        .route("/config", post(conf))
        .route("/stop", post(stop))
        .route("/render-wav", post(render_wav))
        .route("/samples-list", get(samples_list))
        .layer(CorsLayer::permissive()) // Permet les requêtes cross-origin (dev frontend)
        .with_state(state);

    let port = std::env::var("PORT").unwrap_or_else(|_| "4000".to_string());
    println!(
        "\n📡 Serveur prêt sur http://0.0.0.0:{}",
        port
    );
    println!("   Routes :");
    println!("     GET  /              → page d'accueil");
    println!("     POST /play          → lancer la séquence live");
    println!("     POST /config        → modifier la config live");
    println!("     POST /stop          → arrêter la lecture");
    println!("     POST /render-wav    → rendu WAV (batch)");
    println!("     GET  /samples-list  → boucles WAV disponibles\n");

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}
