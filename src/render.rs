/// Render — génération de fichiers WAV à partir d'une grille d'accords.
///
/// Pipeline :
/// 1. `generate_smf_fmt0()` : génère un fichier SMF (Standard MIDI File) Format 0
///    contenant tous les événements MIDI (notes, program changes, tempo) pour
///    les 5 pistes (lead, basse, nappes, drums, accent).
/// 2. `render_wav()`         : appelle FluidSynth en ligne de commande pour
///    convertir le SMF en fichier WAV, puis tronque à la durée exacte.
///
/// Contrairement au live (midi.rs), le render est un batch déterministe :
/// il produit le même WAV pour les mêmes paramètres d'entrée.
///
/// Format SMF produit :
/// - Format 0 (single track, multi-canal)
/// - Résolution : 480 ticks/noire
/// - Événements triés par tick (delta-time encoding)
/// - Meta events : tempo, end-of-track
use std::process::Command;
use crate::patterns::sc;
use crate::walking::{is_minor as is_minor_chord, generate_walking_bass as walking_bass_notes};

/// Résolution : 480 ticks par noire (standard MIDI, compatible tous séquenceurs).
const TICKS_PER_BEAT: u32 = 480;

// ─── Helpers SMF ───────────────────────────────────────────────────────

/// Écrit un entier en VLQ (Variable Length Quantity) — format MIDI standard
/// pour les delta-times et les meta event lengths.
///
/// Chaque byte a le MSB à 1 si un autre byte suit, à 0 si c'est le dernier.
/// Exemple : 127 → 0x7F, 128 → 0x81 0x00
fn write_vlq(buf: &mut Vec<u8>, v: u32) {
    let mut bytes = Vec::new();
    bytes.push((v & 0x7F) as u8);
    let mut v = v >> 7;
    while v > 0 {
        bytes.push(((v & 0x7F) | 0x80) as u8);
        v >>= 7;
    }
    buf.extend(bytes.into_iter().rev());
}

/// Écrit un u16 en big-endian dans le buffer (format MIDI).
fn write_u16(buf: &mut Vec<u8>, v: u16) { buf.extend_from_slice(&v.to_be_bytes()); }

/// Écrit un u32 en big-endian dans le buffer (format MIDI).
fn write_u32(buf: &mut Vec<u8>, v: u32) { buf.extend_from_slice(&v.to_be_bytes()); }

// ─── Canaux MIDI ──────────────────────────────────────────────────────
// Mêmes assignations que dans midi.rs pour la cohérence live/render.
const CH_LEAD: u8 = 0;   // Lead — pompe skank
const CH_BASS: u8 = 2;   // Basse — walking ou tenue
const CH_STR: u8 = 3;    // Nappes (strings/pad)
const CH_ACC: u8 = 4;    // Accent 2&4
const CH_DRUMS: u8 = 9;  // Drums (GM percussions)

// Raccourcis notes drums (mêmes valeurs que dans patterns.rs)
const KICK: u8 = 36;     // Grosse caisse
const SNARE: u8 = 38;    // Caisse claire
const HH: u8 = 42;       // Hi-hat fermée
const RIM: u8 = 37;      // Rimshot

// ─── Config de rendu ─────────────────────────────────────────────────

/// Configuration complète d'un rendu batch.
/// Les champs correspondent aux paramètres envoyés depuis le frontend.
pub struct RenderCfg {
    pub tempo: u32,                 // BPM (120 par défaut)
    pub pattern: String,            // Pattern drums : "rock"|"reggae"|"jazz"|"pop"|"bossa"|"onedrop"
    pub walking: bool,              // Walking bass activée ?
    pub sig: String,                // Signature rythmique, ex "4/4"
    pub lead_inst: u16,             // Program MIDI pour le lead
    pub tracks: [TrackCfg; 5],      // Configuration des 5 pistes
}

/// Configuration d'une piste pour le render.
/// Similaire à midi::TrackCfg mais sans Option (valeurs requises).
#[derive(Clone, Copy)]
pub struct TrackCfg {
    pub channel: u8,    // Canal MIDI
    pub program: u16,   // Program change (instrument GM)
    pub volume: u8,     // Volume (0-127)
    pub mute: bool,     // Mute — si true, la piste est silencieuse
}

impl Default for RenderCfg {
    fn default() -> Self {
        Self {
            tempo: 120, pattern: "rock".into(), walking: false, sig: "4/4".into(), lead_inst: 51,
            tracks: [
                TrackCfg { channel: 0, program: 51, volume: 15, mute: false },
                TrackCfg { channel: 2, program: 33, volume: 40, mute: false },
                TrackCfg { channel: 3, program: 48, volume: 30, mute: false },
                TrackCfg { channel: 9, program: 1, volume: 80, mute: false },
                TrackCfg { channel: 4, program: 2, volume: 20, mute: false },
            ],
        }
    }
}

// ─── SMF Format 0 ─────────────────────────────────────────────────────

/// Événement MIDI avec tick absolu (pour le tri avant sérialisation).
struct Ev {
    tick: u32,          // Tick absolu depuis le début
    bytes: Vec<u8>,     // Message MIDI brut (status + data)
}

/// Ajoute un événement à la liste, avec tick absolu.
fn e(evs: &mut Vec<Ev>, tick: u32, bytes: &[u8]) {
    evs.push(Ev { tick, bytes: bytes.to_vec() });
}

/// Génère un fichier SMF Format 0 à partir de la grille d'accords et de
/// la configuration de rendu.
///
/// # Étapes
/// 1. Meta event : tempo (au tick 0)
/// 2. Program Changes (au tick 0) pour chaque piste
/// 3. Pour chaque accord :
///    a. Lead : pompe skank sur contretemps (8ème, staccato 16ème)
///    b. Basse : walking (4 notes/mesure) ou note tenue
///    c. Nappes : notes tenues sur toute la durée
///    d. Drums : pattern complet selon le style
///    e. Accent : coup sur temps 2&4
/// 4. End-of-track meta event
///
/// Les événements sont d'abord collectés avec des ticks absolus, puis
/// triés et sérialisés en delta-time encoding.
///
/// # Paramètres
/// - `notes_arrays` : notes MIDI pour chaque accord (Vec<u8> = notes de l'accord)
/// - `beats`        : durée en temps de chaque accord
/// - `cfg`          : configuration de rendu
pub fn generate_smf_fmt0(
    notes_arrays: &[Vec<u8>],
    beats: &[f64],
    cfg: &RenderCfg,
) -> Vec<u8> {
    // Tempo en microsecondes par noire (MIDI meta event 0x51)
    let tempo_us = (60_000_000u64 / cfg.tempo.max(1) as u64) as u32;
    let tpb = TICKS_PER_BEAT;      // Ticks par noire
    let eighth = tpb / 2;           // Ticks pour une croche (240)

    // Résoudre la signature rythmique → temps par mesure
    let sig_parts: Vec<&str> = cfg.sig.split('/').collect();
    let beats_per_bar = sig_parts.first()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(4);  // Défaut 4/4

    let mut evs: Vec<Ev> = Vec::new();

    // ── Meta event : Tempo (SMF meta 0x51, 3 bytes big-endian) ──
    e(&mut evs, 0, &[0xFF, 0x51, 0x03,
        ((tempo_us >> 16) & 0xFF) as u8,
        ((tempo_us >> 8)  & 0xFF) as u8,
        (tempo_us & 0xFF) as u8]);

    // ── Program Changes initiaux ─────────────────────────────────
    // Chaque piste reçoit son instrument GM au tick 0.
    for tc in &cfg.tracks {
        // 0xC0 | channel = Program Change, suivi du numéro de program
        e(&mut evs, 0, &[0xC0 | tc.channel, tc.program as u8]);
    }
    // Fallback : si CH_LEAD n'est pas dans tracks, envoyer cfg.lead_inst
    let has_lead = cfg.tracks.iter().any(|t| t.channel == CH_LEAD);
    if !has_lead {
        e(&mut evs, 0, &[0xC0 | CH_LEAD, cfg.lead_inst as u8]);
    }

    // ── Extraire les configs par canal ─────────────────────────────
    let lead_cfg = cfg.tracks.iter().find(|t| t.channel == CH_LEAD);
    let bass_cfg = cfg.tracks.iter().find(|t| t.channel == CH_BASS);
    let str_cfg = cfg.tracks.iter().find(|t| t.channel == CH_STR);
    let drums_cfg = cfg.tracks.iter().find(|t| t.channel == CH_DRUMS);
    let acc_cfg = cfg.tracks.iter().find(|t| t.channel == CH_ACC);

    let lead_mute = lead_cfg.map_or(false, |t| t.mute);
    let bass_mute = bass_cfg.map_or(false, |t| t.mute);
    let str_mute = str_cfg.map_or(false, |t| t.mute);
    let drums_mute = drums_cfg.map_or(false, |t| t.mute);
    let acc_mute = acc_cfg.map_or(false, |t| t.mute);

    let lead_vol = lead_cfg.map_or(80, |t| t.volume);
    let bass_vol = bass_cfg.map_or(90, |t| t.volume);
    let str_vol = str_cfg.map_or(60, |t| t.volume);
    let drums_vol = drums_cfg.map_or(100, |t| t.volume);
    let acc_vol = acc_cfg.map_or(70, |t| t.volume);

    // ── Boucle sur les accords ─────────────────────────────────────
    let mut abs_tick = 0u32;     // Tick absolu courant (accumulé)
    let mut seed: u64 = 0;       // Seed pour la walking bass

    for (ci, notes) in notes_arrays.iter().enumerate() {
        // Nombre de temps que dure cet accord (4.0 = mesure complète)
        let bc = if ci < beats.len() { beats[ci] } else { 4.0 };
        let total_ticks = (bc * tpb as f64) as u32;
        let nq = bc as u32;       // Temps dans cet accord (floor)

        // Accord vide (silence) → skip sans générer de notes
        if notes.is_empty() {
            abs_tick += total_ticks;
            continue;
        }

        let chord_start = abs_tick;            // Tick de début
        let chord_end = abs_tick + total_ticks; // Tick de fin

        // La première note = fondamentale (basse)
        let bass_note = notes[0];
        // Les notes suivantes = chord tones (lead, nappes, accent)
        let chord: &[u8] = if notes.len() > 1 { &notes[1..] } else { &[] };

        // ── Boucle sur les temps de cet accord ─────────────────

        // Lead — pompe skank : staccato sur contretemps 8ème
        // Formule : Note On au tick = beat*240 (milieu du temps), Off 120 ticks plus tard
        // (120 ticks = 1/16ème de note à 480 tpb = durée staccato)
        if !lead_mute {
            let lv = sc(lead_vol, 127); // Vélocité lead scalée
            for b in 0..nq {
                let skank_on = chord_start + b * tpb + eighth; // 8ème offbeat = tick 240
                for &n in chord {
                    // 0x90 | CH_LEAD = Note On
                    e(&mut evs, skank_on, &[0x90 | CH_LEAD, n, lv]);
                }
                let skank_off = skank_on + 120; // Staccato : off 1/16ème plus tard
                for &n in chord {
                    // 0x80 | CH_LEAD = Note Off
                    e(&mut evs, skank_off, &[0x80 | CH_LEAD, n, 64]);
                }
            }
        }

        // Basse — walking bass ou note tenue
        if !bass_mute {
            let bv = sc(bass_vol, 127);
            if cfg.walking && chord.len() >= 1 {
                // Walking bass : 4 notes par mesure
                let next_root = notes_arrays.get(ci + 1)
                    .and_then(|n| n.first()).copied()
                    .or_else(|| notes_arrays.first().and_then(|n| n.first()).copied())
                    .unwrap_or(bass_note);

                let wb_notes = walking_bass_notes(
                    &[bass_note, chord[0], chord.get(1).copied().unwrap_or(bass_note)],
                    next_root, seed,
                    is_minor_chord(&[bass_note, chord[0]]),
                );
                seed = seed.wrapping_add(1);

                for (bi, &bn) in wb_notes.iter().enumerate() {
                    let bt = chord_start + (bi as u32) * tpb; // Une note par temps
                    e(&mut evs, bt, &[0x90 | CH_BASS, bn, bv]); // Note On
                    // Note Off 1 tick avant la note suivante (legato brisé)
                    let off_tick = if bi < 3 {
                        chord_start + ((bi + 1) as u32) * tpb - 1
                    } else {
                        chord_end
                    };
                    e(&mut evs, off_tick, &[0x80 | CH_BASS, bn, 64]); // Note Off
                }
            } else {
                // Note tenue de basse sur toute la durée
                e(&mut evs, chord_start, &[0x90 | CH_BASS, bass_note, bv]);
                e(&mut evs, chord_end, &[0x80 | CH_BASS, bass_note, 64]);
            }
        }

        // Nappes (strings) — notes tenues sur toute la durée de l'accord
        // En reggae, les nappes ne jouent que sur les accords courts.
        let reggae_skip_nappe = cfg.pattern == "reggae" && bc > 1.0;
        if !str_mute && !reggae_skip_nappe {
            let sv = sc(str_vol, 127);
            for &n in chord {
                e(&mut evs, chord_start, &[0x90 | CH_STR, n, sv]);
            }
            for &n in chord {
                e(&mut evs, chord_end, &[0x80 | CH_STR, n, 64]);
            }
        }

        // ── Drums + Accent : par temps ────────────
        // On génère les événements pour chaque temps de l'accord
        for b in 0..nq {
            let on_tick = chord_start + b * tpb;         // Début du temps
            let up_tick = on_tick + eighth;              // Contretemps 8ème
            // Beat absolu pour le pattern (wrap à beats_per_bar)
            let bar_beat = (abs_tick / tpb + b) % beats_per_bar;

            // ── Drums — pattern exact comme dans drum_hit() ─────
            // Chaque pattern définit quels instruments jouent sur quels temps.
            // Voir midi.rs:drum_hit() pour la logique musicale détaillée.
            if !drums_mute {
                let dv = sc(drums_vol, 127);

                // Pre-calculs des vélocités scalées
                let hh_beat = sc(dv, 80);   // HH sur temps (fort)
                let hh_eighth = sc(dv, 65); // HH sur croche (doux)
                let hh55 = sc(dv, 55);
                let hh45 = sc(dv, 45);
                let hh40 = sc(dv, 10);      // HH ghost
                let hh60 = sc(dv, 60);
                let hh65 = sc(dv, 65);

                match cfg.pattern.as_str() {
                    "reggae" => {
                        match bar_beat {
                            0 | 1 | 3 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, HH, hh60]);
                            }
                            2 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, KICK, sc(dv, 120)]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, HH, hh65]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, RIM, sc(dv, 90)]);
                            }
                            _ => {}
                        }
                        e(&mut evs, up_tick, &[0x90 | CH_DRUMS, HH, hh40]);
                    }
                    "jazz" => {
                        let bb2 = bar_beat % 8; // Le jazz se répète sur 2 mesures
                        match bb2 {
                            0 | 2 | 6 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, 51, hh60]); // Ride
                            }
                            4 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, 51, hh60]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, 44, sc(dv, 40)]); // HH pedale
                            }
                            7 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, 51, hh60]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, 44, sc(dv, 40)]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, RIM, sc(dv, 50)]);
                            }
                            _ => {}
                        }
                        e(&mut evs, up_tick, &[0x90 | CH_DRUMS, HH, 35]);
                    }
                    "pop" => {
                        match bar_beat % 4 {
                            0 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, KICK, sc(dv, 85)]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, HH, sc(dv, 50)]);
                            }
                            1 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, SNARE, sc(dv, 70)]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, HH, sc(dv, 50)]);
                            }
                            2 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, KICK, sc(dv, 75)]);
                            }
                            3 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, SNARE, sc(dv, 65)]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, HH, sc(dv, 50)]);
                            }
                            _ => {}
                        }
                        e(&mut evs, up_tick, &[0x90 | CH_DRUMS, HH, sc(dv, 45)]);
                    }
                    "bossa" => {
                        match bar_beat % 4 {
                            0 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, KICK, sc(dv, 55)]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, HH, hh45]);
                            }
                            1 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, SNARE, sc(dv, 30)]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, HH, hh45]);
                            }
                            2 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, KICK, sc(dv, 60)]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, HH, hh45]);
                            }
                            3 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, KICK, sc(dv, 50)]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, HH, hh45]);
                            }
                            _ => {}
                        }
                        e(&mut evs, up_tick, &[0x90 | CH_DRUMS, HH, hh40]);
                    }
                    "onedrop" => {
                        match bar_beat % 4 {
                            0 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, KICK, sc(dv, 90)]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, HH, hh55]);
                            }
                            1 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, HH, hh40]);
                            }
                            2 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, KICK, sc(dv, 90)]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, RIM, sc(dv, 65)]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, HH, hh45]);
                            }
                            3 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, HH, hh55]);
                            }
                            _ => {}
                        }
                        e(&mut evs, up_tick, &[0x90 | CH_DRUMS, HH, hh40]);
                    }
                    // Pattern par défaut : ROCK
                    _ => {
                        match bar_beat % 4 {
                            0 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, KICK, sc(dv, 90)]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, HH, hh_beat]);
                            }
                            1 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, SNARE, sc(dv, 75)]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, HH, hh_beat]);
                            }
                            2 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, KICK, sc(dv, 80)]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, HH, hh_beat]);
                            }
                            3 => {
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, SNARE, sc(dv, 70)]);
                                e(&mut evs, on_tick, &[0x90 | CH_DRUMS, HH, hh_beat]);
                            }
                            _ => {}
                        }
                        e(&mut evs, up_tick, &[0x90 | CH_DRUMS, HH, hh_eighth]);
                    }
                }
            }

            // ── Accent (temps 2&4) ──────────────────────
            // Coup sec de Bright Acoustic Piano (canal 4) sur les temps
            // faibles (backbeat), façon ska/rocksteady.
            if !acc_mute && (b == 1 || b == 3) {
                let av = sc(acc_vol, 127);
                for &n in chord {
                    e(&mut evs, on_tick, &[0x90 | CH_ACC, n, av]);
                    // Note Off quasi immédiat (1 tick plus tard) pour un effet
                    // percussif, pas de note tenue
                    e(&mut evs, on_tick + 60, &[0x80 | CH_ACC, n, 64]); // Off 1/8ème plus tard
                }
            }
        }

        // Mise à jour du tick absolu pour le prochain accord
        abs_tick = chord_end;
    }

    // ── Sérialisation SMF ──────────────────────────────────────────
    // 1. Trier les événements par tick absolu
    evs.sort_by_key(|e| e.tick);

    // 2. Encoder en delta-time : chaque événement écrit la différence
    //    de ticks depuis le précédent (VLQ) + le message MIDI brut.
    let mut track_data = Vec::new();
    let mut prev_tick = 0u32;
    for ev in &evs {
        let delta = ev.tick - prev_tick;
        write_vlq(&mut track_data, delta);
        track_data.extend_from_slice(&ev.bytes);
        prev_tick = ev.tick;
    }

    // 3. End-of-Track meta event (obligatoire pour un SMF valide)
    write_vlq(&mut track_data, 0);
    track_data.extend_from_slice(&[0xFF, 0x2F, 0x00]);

    // 4. Assemblage du fichier SMF complet
    let mut smf = Vec::new();
    // Entête MThd
    smf.extend_from_slice(b"MThd");
    write_u32(&mut smf, 6);          // Taille de l'entête = 6 bytes
    write_u16(&mut smf, 0);          // Format 0 (1 track)
    write_u16(&mut smf, 1);          // 1 piste
    write_u16(&mut smf, tpb as u16); // Résolution : ticks par noire
    // Track MTrk
    smf.extend_from_slice(b"MTrk");
    write_u32(&mut smf, track_data.len() as u32);
    smf.extend_from_slice(&track_data);

    smf
}

// ─── Render WAV ──────────────────────────────────────────────────────

/// Tronque un WAV à la durée attendue (`expected_sec` secondes).
///
/// FluidSynth peut produire un WAV légèrement plus long que la durée
/// musicale à cause de la résonance de la SoundFont.  Cette fonction
/// coupe le surplus pour que le fichier corresponde exactement à la
/// durée calculée (total_beats * 60 / BPM).
fn trim_to_duration(wav: &[u8], expected_sec: f64) -> Vec<u8> {
    use hound::WavReader;
    let Ok(mut reader) = WavReader::new(std::io::Cursor::new(wav)) else {
        return wav.to_vec();
    };
    let spec = reader.spec();
    let samples: Vec<i16> = reader.samples::<i16>().filter_map(|s| s.ok()).collect();
    if samples.is_empty() { return wav.to_vec(); }

    // Nombre d'échantillons attendus = durée × sample_rate × canaux
    let expected_samples = (expected_sec * spec.sample_rate as f64).round() as usize
        * spec.channels as usize;
    let end = expected_samples.min(samples.len());

    // Ré-écrire le WAV tronqué
    let mut out = Vec::new();
    if let Ok(mut w) = hound::WavWriter::new(std::io::Cursor::new(&mut out), spec) {
        for &s in &samples[..end] { let _ = w.write_sample(s); }
        let _ = w.finalize();
    }
    if out.is_empty() { wav.to_vec() } else { out }
}

/// Lance FluidSynth en ligne de commande pour convertir un SMF en WAV.
///
/// # Pipeline
/// 1. Écrit le SMF dans `/tmp/chordj_render.mid`
/// 2. Appelle `fluidsynth -F <wav> -T wav -g 1.0 -n -i <soundfont> <mid>`
/// 3. Lit le WAV produit
/// 4. Nettoie les fichiers temporaires
/// 5. Tronque le WAV à la durée exacte
///
/// # Paramètres
/// - `smf` : contenu du fichier SMF (bytes)
/// - `soundfont` : chemin vers le fichier .sf3 (SoundFont)
/// - `duration_sec` : durée attendue en secondes (pour le trim)
///
/// # Erreurs
/// - Retourne `Err(String)` si FluidSynth n'est pas installé, si le SMF
///   est invalide, ou si le fichier WAV ne peut pas être écrit/lu.
pub fn render_wav(smf: &[u8], soundfont: &str, duration_sec: f64) -> Result<Vec<u8>, String> {
    let mid_path = std::env::temp_dir().join("chordj_render.mid");
    let wav_path = std::env::temp_dir().join("chordj_render.wav");

    // Étape 1 : écrire le SMF temporaire
    std::fs::write(&mid_path, smf).map_err(|e| format!("Impossible d'écrire le MIDI temporaire : {}", e))?;

    // Étape 2 : lancer FluidSynth
    // Options :
    //   -F <wav> : fichier de sortie WAV
    //   -T wav   : format de sortie
    //   -g 1.0   : gain (volume) de rendu
    //   -n       : ne pas charger les defaults
    //   -i       : mode interactif (permet de charger un seul fichier)
    let output = Command::new("fluidsynth")
        .arg("-F").arg(&wav_path)
        .arg("-T").arg("wav")
        .arg("-g").arg("1.0")
        .arg("-n").arg("-i")
        .arg(soundfont)
        .arg(&mid_path)
        .output()
        .map_err(|e| format!("Impossible d'exécuter fluidsynth : {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = std::fs::remove_file(&mid_path);
        return Err(format!("FluidSynth a échoué : {}", stderr));
    }

    // Conserver les logs stderr pour le diagnostic
    if let Ok(s) = String::from_utf8(output.stderr) {
        let _ = std::fs::write(std::env::temp_dir().join("chordj_render.log"), &s);
    }

    // Étape 3 : lire le WAV produit
    let wav = std::fs::read(&wav_path)
        .map_err(|e| format!("Impossible de lire le WAV rendu : {}", e))?;

    // Étape 4 : nettoyage
    let _ = std::fs::remove_file(&mid_path);
    let _ = std::fs::remove_file(&wav_path);

    // Étape 5 : tronquer à la durée exacte
    Ok(trim_to_duration(&wav, duration_sec))
}
