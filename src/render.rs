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
/// - Résolution : 288 ticks/noire (divisible par 3 et 4 → triolets 1/12, 1/6,
///   1/3 et sextolets 1/24, 1/18 exacts, ainsi que toutes les subdivisions
///   binaires jusqu'au 1/32)
/// - Événements triés par tick (delta-time encoding)
/// - Meta events : tempo, end-of-track
use std::process::Command;
use serde::Serialize;
use crate::patterns::sc;
use crate::walking::{is_minor as is_minor_chord, generate_walking_bass as walking_bass_notes};

/// Résolution : 288 ticks par noire (multiple commun de 32, 24, 18, 16, 12,
/// 8, 6, 4, 3, 2, 1 → tous les snaps du PianoRoll sont exacts, notamment les
/// triolets). 288 = 2^5 × 3^2, divisible par 18 et 24.
const TICKS_PER_BEAT: u32 = 288;

use std::sync::atomic::{AtomicU64, Ordering};
static RENDER_TAG: AtomicU64 = AtomicU64::new(0);

/// Tag UNIQUE par appel de rendu — évite la course sur les fichiers
/// temporaires quand deux rendus se chevauchent (scrubs rapides en mode
/// séparé, render-tracks + render-wav simultanés…). Avant : noms fixes
/// /tmp/chordj_render.mid|.wav → le premier appel supprimait les fichiers
/// pendant que le second les lisait (« No such file or directory »).
fn next_render_tag() -> String {
    let n = RENDER_TAG.fetch_add(1, Ordering::Relaxed);
    format!("chordj_render_{}_{}", std::process::id(), n)
}

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
    pub tracks: Vec<TrackCfg>,      // Configuration des pistes (DYNAMIQUE : ajout/suppression)
}

/// Configuration d'une piste pour le render.
/// Similaire à midi::TrackCfg mais sans Option (valeurs requises).
#[derive(Clone, Copy)]
pub struct TrackCfg {
    pub channel: u8,    // Canal MIDI
    pub program: u16,   // Program change (instrument GM)
    pub volume: u8,     // Volume (0-127)
    pub mute: bool,     // Mute — si true, la piste est silencieuse
    /// Piste percussion (kit drums) sur un canal ≠ 9 : programmée via la
    /// banque percussion GM2 (bank select 128) + kit Standard (program 0).
    pub drums: bool,
    /// Bank select drums (CC0 MSB / CC32 LSB) — kits alternatifs (ex. Roland).
    pub bank_msb: u8,
    pub bank_lsb: u8,
    pub fx: crate::dsp::Fx,  // Effets par piste (reverb/chorus/delay/drive)
}

impl Default for RenderCfg {
    fn default() -> Self {
        Self {
            tempo: 120, pattern: "rock".into(), walking: false, sig: "4/4".into(), lead_inst: 51,
            tracks: vec![
                TrackCfg { channel: 0, program: 51, volume: 60, mute: false, drums: false, bank_msb: 0, bank_lsb: 0, fx: Default::default() },
                TrackCfg { channel: 2, program: 33, volume: 70, mute: false, drums: false, bank_msb: 0, bank_lsb: 0, fx: Default::default() },
                TrackCfg { channel: 3, program: 48, volume: 60, mute: false, drums: false, bank_msb: 0, bank_lsb: 0, fx: Default::default() },
                TrackCfg { channel: 9, program: 1, volume: 90, mute: false, drums: false, bank_msb: 0, bank_lsb: 0, fx: Default::default() },
                TrackCfg { channel: 4, program: 2, volume: 50, mute: false, drums: false, bank_msb: 0, bank_lsb: 0, fx: Default::default() },
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
/// 3. Notes générées par `generate_notes()` (lead, basse, nappes, drums, accent)
/// 4. End-of-track meta event
///
/// Les événements sont triés par tick absolu puis sérialisés en delta-time.
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

    let mut evs: Vec<Ev> = Vec::new();

    // ── Meta event : Tempo (SMF meta 0x51, 3 bytes big-endian) ──
    e(&mut evs, 0, &[0xFF, 0x51, 0x03,
        ((tempo_us >> 16) & 0xFF) as u8,
        ((tempo_us >> 8)  & 0xFF) as u8,
        (tempo_us & 0xFF) as u8]);

    // ── Program Changes initiaux ─────────────────────────────────
    // Chaque piste reçoit son instrument GM au tick 0. Les pistes
    // percussion sur d'autres canaux n'ont pas besoin de programmation
    // (leurs notes sont redirigées vers le canal drums natif 9).
    for tc in &cfg.tracks {
        // 0xC0 | channel = Program Change, suivi du numéro de program
        e(&mut evs, 0, &[0xC0 | tc.channel, tc.program as u8]);
    }
    // Fallback : si CH_LEAD n'est pas dans tracks, envoyer cfg.lead_inst
    let has_lead = cfg.tracks.iter().any(|t| t.channel == CH_LEAD);
    if !has_lead {
        e(&mut evs, 0, &[0xC0 | CH_LEAD, cfg.lead_inst as u8]);
    }

    // ── Notes (lead, basse, nappes, drums, accent) ───────────────
    // La génération musicale est centralisée dans `generate_notes()`,
    // partagée avec le PianoRoll via l'endpoint /render-notes.
    let notes = generate_notes(notes_arrays, beats, cfg);
    for n in &notes {
        let start_tick = (n.start_time * tpb as f64) as u32;
        let end_tick = ((n.start_time + n.duration) * tpb as f64) as u32;
        e(&mut evs, start_tick, &[0x90 | n.channel, n.pitch, n.velocity]);
        e(&mut evs, end_tick, &[0x80 | n.channel, n.pitch, 64]);
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

/// Génère les notes MIDI structurées de toutes les pistes (mode classique).
///
/// C'est la source de vérité musicale partagée entre :
/// - `generate_smf_fmt0()` : rendu WAV classique (conversion en événements SMF)
/// - Endpoint `POST /render-notes` : pré-remplissage des PianoRolls frontend
///
/// Retourne des notes avec positions/durées en **beats**, dans le même format
/// que les `custom_notes` envoyées par le PianoRoll (channel, start_time,
/// pitch, duration, velocity).
///
/// # Étapes (par accord)
/// 1. Lead : pompe skank sur contretemps (8ème, staccato 16ème)
/// 2. Basse : walking (4 notes/mesure) ou note tenue
/// 3. Nappes : notes tenues sur toute la durée
/// 4. Drums : pattern complet selon le style
/// 5. Accent : coup sur temps 2&4
pub fn generate_notes(
    notes_arrays: &[Vec<u8>],
    beats: &[f64],
    cfg: &RenderCfg,
) -> Vec<CustomNote> {
    let tpb = TICKS_PER_BEAT as f64;
    // Résoudre la signature rythmique → temps par mesure
    let sig_parts: Vec<&str> = cfg.sig.split('/').collect();
    let beats_per_bar = sig_parts.first()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(4);  // Défaut 4/4

    // ── Extraire les configs par canal ─────────────────────────────
    let lead_cfg = cfg.tracks.iter().find(|t| t.channel == CH_LEAD);
    let bass_cfg = cfg.tracks.iter().find(|t| t.channel == CH_BASS);
    let str_cfg = cfg.tracks.iter().find(|t| t.channel == CH_STR);
    let drums_cfg = cfg.tracks.iter().find(|t| t.channel == CH_DRUMS);
    let acc_cfg = cfg.tracks.iter().find(|t| t.channel == CH_ACC);

    // Canal ABSENT de la config = piste supprimée côté utilisateur → MUET.
    // (map_or(true) : absence = muet. L'ancien map_or(false) laissait jouer
    // la piste par défaut — inoffensif tant que le frontend envoyait toujours
    // les 5 canaux, mais faux avec des pistes dynamiques.)
    let lead_mute = lead_cfg.map_or(true, |t| t.mute);
    let bass_mute = bass_cfg.map_or(true, |t| t.mute);
    let str_mute = str_cfg.map_or(true, |t| t.mute);
    let drums_mute = drums_cfg.map_or(true, |t| t.mute);
    let acc_mute = acc_cfg.map_or(true, |t| t.mute);

    let lead_vol = lead_cfg.map_or(80, |t| t.volume);
    let bass_vol = bass_cfg.map_or(90, |t| t.volume);
    let str_vol = str_cfg.map_or(60, |t| t.volume);
    let drums_vol = drums_cfg.map_or(100, |t| t.volume);
    let acc_vol = acc_cfg.map_or(70, |t| t.volume);

    let mut out: Vec<CustomNote> = Vec::new();
    let mut abs_beats = 0.0f64;   // Position absolue en beats
    let mut seed: u64 = 0;        // Seed pour la walking bass

    for (ci, notes) in notes_arrays.iter().enumerate() {
        // Nombre de temps que dure cet accord (4.0 = mesure complète)
        let bc = if ci < beats.len() { beats[ci] } else { 4.0 };
        let nq = bc as u32;       // Temps entiers dans cet accord (floor)
        // Accords plus courts qu'1 temps (notation 6:, 8:, 12:…) : au moins
        // un slot d'attaque pour que lead et drums ne soient pas muets.
        let slots = if bc > 0.0 && nq == 0 { 1 } else { nq };
        let short_chord = bc < 1.0;  // durées < 1 temps : traitement spécifique

        // Accord vide (silence) → skip sans générer de notes
        if notes.is_empty() {
            abs_beats += bc;
            continue;
        }

        // La première note = fondamentale (basse)
        let bass_note = notes[0];
        // Les notes suivantes = chord tones (lead, nappes, accent)
        let chord: &[u8] = if notes.len() > 1 { &notes[1..] } else { &[] };

        // ── Lead — pompe skank : staccato sur contretemps 8ème ──
        // Start = beat + 0.5 (contretemps), durée 0.25 (1/16 staccato).
        // Accord < 1 temps : plaqué au début (le contretemps déborderait).
        if !lead_mute {
            let lv = sc(lead_vol, 127);
            if short_chord {
                for &n in chord {
                    out.push(CustomNote {
                        channel: CH_LEAD, start_time: abs_beats, pitch: n,
                        duration: bc.min(0.25), velocity: lv,
                    });
                }
            } else {
                for b in 0..nq {
                    let start = abs_beats + b as f64 + 0.5;
                    for &n in chord {
                        out.push(CustomNote {
                            channel: CH_LEAD, start_time: start, pitch: n,
                            duration: 0.25, velocity: lv,
                        });
                    }
                }
            }
        }

        // ── Basse — walking bass ou note tenue ──
        if !bass_mute {
            let bv = sc(bass_vol, 127);
            if cfg.walking && chord.len() >= 1 {
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

                // Durée identique au SMF historique : off 1 tick avant la
                // note suivante (legato brisé). Dernière note : jusqu'à la
                // fin de l'accord. Si la durée est négative (accord < 3
                // temps), la note était inaudible dans l'ancien rendu → on
                // ne l'émet pas (comportement identique).
                // Accord < 1 temps : une seule note tenue sur bc (la walking
                // complète déborderait sur l'accord suivant).
                let short = (TICKS_PER_BEAT as f64 - 1.0) / tpb;
                let nb = if short_chord { 1 } else { 4 };
                for (bi, &bn) in wb_notes.iter().enumerate().take(nb) {
                    let dur = if short_chord {
                        bc
                    } else if bi < 3 {
                        short
                    } else {
                        bc - 3.0
                    };
                    if dur > 0.0 {
                        out.push(CustomNote {
                            channel: CH_BASS, start_time: abs_beats + bi as f64,
                            pitch: bn, duration: dur, velocity: bv,
                        });
                    }
                }
            } else {
                // Note tenue de basse sur toute la durée
                out.push(CustomNote {
                    channel: CH_BASS, start_time: abs_beats, pitch: bass_note,
                    duration: bc, velocity: bv,
                });
            }
        }

        // ── Nappes (strings) — notes tenues sur toute la durée ──
        // En reggae, les nappes ne jouent que sur les accords courts.
        let reggae_skip_nappe = cfg.pattern == "reggae" && bc > 1.0;
        if !str_mute && !reggae_skip_nappe {
            let sv = sc(str_vol, 127);
            for &n in chord {
                out.push(CustomNote {
                    channel: CH_STR, start_time: abs_beats, pitch: n,
                    duration: bc, velocity: sv,
                });
            }
        }

        // ── Drums + Accent : par temps ────────────
        for b in 0..slots {
            // Beat absolu pour le pattern (wrap à beats_per_bar)
            let bar_beat = (abs_beats as u32 + b) % beats_per_bar;
            let t0 = abs_beats + b as f64;   // Début du temps
            let up = t0 + 0.5;               // Contretemps 8ème
            // Accord < 1 temps : pas de contretemps (il déborderait de l'accord)
            let play_up = !short_chord;

            // ── Drums — pattern exact comme drum_hit() ──────────
            if !drums_mute {
                let dv = sc(drums_vol, 127);

                // Pre-calculs des vélocités scalées (identiques au SMF)
                let hh_beat = sc(dv, 80);   // HH sur temps (fort)
                let hh_eighth = sc(dv, 65); // HH sur croche (doux)
                let hh55 = sc(dv, 55);
                let hh45 = sc(dv, 45);
                let hh40 = sc(dv, 10);      // HH ghost
                let hh60 = sc(dv, 60);
                let hh65 = sc(dv, 65);

                // Durée drums : 1/16 (0.25 beat) — one-shot
                let mut hit = |p: u8, v: u8, at: f64| {
                    out.push(CustomNote {
                        channel: CH_DRUMS, start_time: at, pitch: p,
                        duration: 0.25, velocity: v,
                    });
                };

                match cfg.pattern.as_str() {
                    "reggae" => {
                        match bar_beat {
                            0 | 1 | 3 => { hit(HH, hh60, t0); }
                            2 => {
                                hit(KICK, sc(dv, 120), t0);
                                hit(HH, hh65, t0);
                                hit(RIM, sc(dv, 90), t0);
                            }
                            _ => {}
                        }
                        if play_up { hit(HH, hh40, up); }
                    }
                    "jazz" => {
                        let bb2 = bar_beat % 8; // Le jazz se répète sur 2 mesures
                        match bb2 {
                            0 | 2 | 6 => { hit(51, hh60, t0); } // Ride
                            4 => {
                                hit(51, hh60, t0);
                                hit(44, sc(dv, 40), t0); // HH pedale
                            }
                            7 => {
                                hit(51, hh60, t0);
                                hit(44, sc(dv, 40), t0);
                                hit(RIM, sc(dv, 50), t0);
                            }
                            _ => {}
                        }
                        if play_up { hit(HH, 35, up); }
                    }
                    "pop" => {
                        match bar_beat % 4 {
                            0 => {
                                hit(KICK, sc(dv, 85), t0);
                                hit(HH, sc(dv, 50), t0);
                            }
                            1 => {
                                hit(SNARE, sc(dv, 70), t0);
                                hit(HH, sc(dv, 50), t0);
                            }
                            2 => { hit(KICK, sc(dv, 75), t0); }
                            3 => {
                                hit(SNARE, sc(dv, 65), t0);
                                hit(HH, sc(dv, 50), t0);
                            }
                            _ => {}
                        }
                        if play_up { hit(HH, sc(dv, 45), up); }
                    }
                    "bossa" => {
                        match bar_beat % 4 {
                            0 => {
                                hit(KICK, sc(dv, 55), t0);
                                hit(HH, hh45, t0);
                            }
                            1 => {
                                hit(SNARE, sc(dv, 30), t0);
                                hit(HH, hh45, t0);
                            }
                            2 => {
                                hit(KICK, sc(dv, 60), t0);
                                hit(HH, hh45, t0);
                            }
                            3 => {
                                hit(KICK, sc(dv, 50), t0);
                                hit(HH, hh45, t0);
                            }
                            _ => {}
                        }
                        if play_up { hit(HH, hh40, up); }
                    }
                    "onedrop" => {
                        match bar_beat % 4 {
                            0 => {
                                hit(KICK, sc(dv, 90), t0);
                                hit(HH, hh55, t0);
                            }
                            1 => { hit(HH, hh40, t0); }
                            2 => {
                                hit(KICK, sc(dv, 90), t0);
                                hit(RIM, sc(dv, 65), t0);
                                hit(HH, hh45, t0);
                            }
                            3 => { hit(HH, hh55, t0); }
                            _ => {}
                        }
                        if play_up { hit(HH, hh40, up); }
                    }
                    // Pattern par défaut : ROCK
                    _ => {
                        match bar_beat % 4 {
                            0 => {
                                hit(KICK, sc(dv, 90), t0);
                                hit(HH, hh_beat, t0);
                            }
                            1 => {
                                hit(SNARE, sc(dv, 75), t0);
                                hit(HH, hh_beat, t0);
                            }
                            2 => {
                                hit(KICK, sc(dv, 80), t0);
                                hit(HH, hh_beat, t0);
                            }
                            3 => {
                                hit(SNARE, sc(dv, 70), t0);
                                hit(HH, hh_beat, t0);
                            }
                            _ => {}
                        }
                        if play_up { hit(HH, hh_eighth, up); }
                    }
                }
            }

            // ── Accent (temps 2&4) ──────────────────────
            // Coup sec de Bright Acoustic Piano (canal 4) sur les temps
            // faibles (backbeat), façon ska/rocksteady. Durée 1/8 (60 ticks).
            if !acc_mute && (b == 1 || b == 3) {
                let av = sc(acc_vol, 127);
                for &n in chord {
                    out.push(CustomNote {
                        channel: CH_ACC, start_time: t0, pitch: n,
                        duration: 0.125, velocity: av,
                    });
                }
            }
        }

        // Position absolue pour le prochain accord
        abs_beats += bc;
    }

    out
}

// ─── Render WAV ──────────────────────────────────────────────────────

/// Normalise un WAV au pic cible (-6 dBFS) puis applique le master volume.
///
/// MÊME règle que `render_wav_mixed` et `render-tracks` (normalisation au
/// pic 0,5 puis master/127) : le niveau d'un rendu ne dépend plus du chemin
/// emprunté (FX on/off) ni du nombre de pistes. Avant : gain fixe ×3 — les
/// rendus simple et par piste ne sonnaient pas au même niveau.
fn apply_gain(wav: &[u8], master_vol: u8) -> Vec<u8> {
    use hound::WavReader;
    let Ok(mut reader) = WavReader::new(std::io::Cursor::new(wav)) else {
        return wav.to_vec();
    };
    let spec = reader.spec();
    let samples: Vec<i16> = reader.samples::<i16>().filter_map(|s| s.ok()).collect();
    if samples.is_empty() { return wav.to_vec(); }

    // Pic du mix → normalisation à 0,5 (-6 dBFS), puis gain du master.
    let mut pic = 0f32;
    for &s in &samples {
        pic = pic.max((s as f32).abs() / 32768.0);
    }
    let norm = if pic > 1e-6 { 0.5 / pic } else { 1.0 };
    let gain = norm * (master_vol as f32 / 127.0);

    let mut out = Vec::new();
    if let Ok(mut w) = hound::WavWriter::new(std::io::Cursor::new(&mut out), spec) {
        for &s in &samples {
            let v = (s as f32 * gain).round().clamp(-32768.0, 32767.0) as i16;
            let _ = w.write_sample(v);
        }
        let _ = w.finalize();
    }
    if out.is_empty() { wav.to_vec() } else { out }
}

/// Applique un fade-out linéaire sur la fin du WAV.
///
/// Le rendu est tronqué à la durée musicale exacte (nécessaire pour que
/// la boucle reste synchrone), mais les queues de notes/réverbération
/// dépassent → coupure nette (clic) à la boucle. Un fade-out court
/// (80 ms) rend la transition douce sans créer de silence perceptible.
fn fade_out_wav(wav: &[u8], fade_ms: u64) -> Vec<u8> {
    use hound::WavReader;
    let Ok(mut reader) = WavReader::new(std::io::Cursor::new(wav)) else {
        return wav.to_vec();
    };
    let spec = reader.spec();
    let samples: Vec<i16> = reader.samples::<i16>().filter_map(|s| s.ok()).collect();
    if samples.is_empty() { return wav.to_vec(); }

    let fade_samples = (fade_ms as usize) * spec.sample_rate as usize
        * spec.channels as usize / 1000; // ms → secondes !
    let fade_samples = fade_samples.min(samples.len());
    let start = samples.len() - fade_samples;

    let mut out = Vec::new();
    if let Ok(mut w) = hound::WavWriter::new(std::io::Cursor::new(&mut out), spec) {
        for (i, &s) in samples.iter().enumerate() {
            let v = if i >= start {
                let t = (i - start) as f64 / fade_samples as f64; // 0→1
                (s as f64 * (1.0 - t)).round() as i16
            } else {
                s
            };
            let _ = w.write_sample(v);
        }
        let _ = w.finalize();
    }
    if out.is_empty() { wav.to_vec() } else { out }
}

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

/// Lance FluidSynth en ligne de commande pour convertir un SMF en WAV
/// (sans gain master ni fade : traitement du mix fait par l'appelant).
///
/// # Pipeline
/// 1. Écrit le SMF dans un fichier temporaire UNIQUE (`chordj_render_<pid>_<n>.mid`)
/// 2. Appelle `fluidsynth -F <wav> -T wav -g 1.0 -n -i <soundfont> <mid>`
/// 3. Lit le WAV produit
/// 4. Nettoie les fichiers temporaires
/// 5. Tronque le WAV à la durée exacte
pub fn render_wav_raw(smf: &[u8], soundfont: &str, duration_sec: f64) -> Result<Vec<u8>, String> {
    // Noms UNIQUES par appel : deux rendus simultanés ne se marchent plus
    // dessus (avant : /tmp/chordj_render.mid|.wav partagés → « No such file »).
    let tag = next_render_tag();
    let mid_path = std::env::temp_dir().join(format!("{}.mid", tag));
    let wav_path = std::env::temp_dir().join(format!("{}.wav", tag));

    // Étape 1 : écrire le SMF temporaire
    std::fs::write(&mid_path, smf).map_err(|e| format!("Impossible d'écrire le MIDI temporaire : {}", e))?;

    // Étape 2 : lancer FluidSynth
    // Options :
    //   -F <wav> : fichier de sortie WAV
    //   -T wav   : format de sortie
    //   -g 0.5   : gain de rendu conservateur — garantit que le mix brut
    //              ne sature JAMAIS (normalisation au pic ensuite)
    //   -n       : ne pas charger les defaults
    //   -i       : mode interactif (permet de charger un seul fichier)
    let output = Command::new("fluidsynth")
        .arg("-F").arg(&wav_path)
        .arg("-T").arg("wav")
        .arg("-g").arg("0.5")
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

    // Étape 5 : tronquer à la durée exacte (le gain/fade sont appliqués
    // par l'appelant : rendu unique → render_wav ; rendu par piste →
    // render_wav_mixed après le mixage).
    Ok(trim_to_duration(&wav, duration_sec))
}

/// Rendu simple (chemin historique) : SMF unique multi-canaux → WAV,
/// puis gain master (×3 normalisé) et micro fade-out.
pub fn render_wav(smf: &[u8], soundfont: &str, duration_sec: f64, master_vol: u8) -> Result<Vec<u8>, String> {
    let raw = render_wav_raw(smf, soundfont, duration_sec)?;
    let normalized = apply_gain(&raw, master_vol);
    Ok(fade_out_wav(&normalized, 30))
}

/// Résultat d'un rendu de piste individuelle (bounce multitrack).
pub struct RenderedTrack {
    pub channel: u8,
    pub wav: Vec<u8>,
}

/// Retire le début d'un WAV (offset en secondes) — utilisé par le repli
/// mixé de navig-play pour démarrer la lecture à la position demandée
/// (le clic reste mélangé, les accents de mesure restent à leur place).
pub fn slice_wav_from(wav: &[u8], start_sec: f64) -> Vec<u8> {
    if start_sec <= 0.0 {
        return wav.to_vec();
    }
    use hound::WavReader;
    let Ok(mut reader) = WavReader::new(std::io::Cursor::new(wav)) else {
        return wav.to_vec();
    };
    let spec = reader.spec();
    let samples: Vec<i16> = reader.samples::<i16>().filter_map(|s| s.ok()).collect();
    let ch = spec.channels.max(1) as usize;
    let start = ((start_sec * spec.sample_rate as f64) as usize) * ch;
    if start >= samples.len() {
        return wav.to_vec(); // garde-fou : offset au-delà de la durée
    }
    let mut out = Vec::new();
    if let Ok(mut w) = hound::WavWriter::new(std::io::Cursor::new(&mut out), spec) {
        for &s in &samples[start..] {
            let _ = w.write_sample(s);
        }
        let _ = w.finalize();
    }
    if out.is_empty() {
        wav.to_vec()
    } else {
        out
    }
}

/// Construit la liste des pistes à rendre : pistes actives (non mutées) +
/// piste drums par défaut si des notes canal 9 existent sans piste native
/// (piste drums sur canal ≠ 9 supprimée → les notes redirigées vers le 9
/// seraient perdues sinon, alors qu'elles jouent dans le rendu simple SMF
/// et en MIDI).
fn render_tracks_list(cfg: &RenderCfg, all_notes: &[CustomNote]) -> Vec<TrackCfg> {
    let mut tracks: Vec<TrackCfg> = cfg.tracks.iter().filter(|t| !t.mute).cloned().collect();
    let has_ch9 = tracks.iter().any(|t| t.channel == CH_DRUMS);
    if !has_ch9 && all_notes.iter().any(|n| n.channel == CH_DRUMS) {
        tracks.push(TrackCfg {
            channel: CH_DRUMS, program: 1, volume: 127, mute: false,
            drums: false, bank_msb: 0, bank_lsb: 0, fx: Default::default(),
        });
    }
    tracks
}

/// Rendu « par piste » SANS mixage (bounce multitrack pour le mode PostProd).
///
/// Chaque piste active est rendue séparément (SMF mono-piste → FluidSynth →
/// WAV), passe dans sa chaîne d'effets (dsp::apply_fx : overdrive → delay →
/// chorus → reverb), puis retournée telle quelle : les niveaux relatifs entre
/// pistes sont préservés (le gain master et la normalisation au pic sont
/// appliqués par l'appelant — mix pour `/render-wav`, frontend pour PostProd).
///
/// Les notes (`all_notes`) doivent être DÉJÀ scalées par volume de piste
/// (fait en amont : generate_notes pour le classique, scaling custom).
pub fn render_tracks_individual(
    all_notes: &[CustomNote],
    cfg: &RenderCfg,
    soundfont: &str,
    duration_sec: f64,
) -> Result<Vec<RenderedTrack>, String> {
    // Delay musical : une croche (0.5 beat) → 30000/tempo ms
    let delay_ms = (30_000u32 / cfg.tempo.max(20)).max(1);
    let tempo = cfg.tempo.max(20) as u16;

    let mut out: Vec<RenderedTrack> = Vec::new();
    let tracks = render_tracks_list(cfg, all_notes);
    for t in &tracks {
        let notes: Vec<CustomNote> = all_notes
            .iter()
            .filter(|n| n.channel == t.channel)
            .cloned()
            .collect();
        if notes.is_empty() {
            continue;
        }

        // SMF de la piste seule (program change + notes du canal)
        let smf = generate_smf_from_custom(&notes, std::slice::from_ref(t), tempo);
        let wav = render_wav_raw(&smf, soundfont, duration_sec)?;

        // Chaîne d'effets de la piste (si réglée)
        let wav = if t.fx.is_off() { wav } else { crate::dsp::apply_fx(&wav, &t.fx, delay_ms) };

        out.push(RenderedTrack { channel: t.channel, wav });
    }
    Ok(out)
}

/// Rendu « par piste » mixé : rend chaque piste séparément (voir
/// `render_tracks_individual`), additionne les WAVs, normalise au pic
/// (~0.5, -6 dBFS) pour un niveau stable quel que soit le nombre de pistes,
/// applique le master volume, et termine par un fade-out final.
pub fn render_wav_mixed(
    all_notes: &[CustomNote],
    cfg: &RenderCfg,
    soundfont: &str,
    duration_sec: f64,
    master_vol: u8,
) -> Result<Vec<u8>, String> {
    use hound::{WavReader, WavSpec, WavWriter, SampleFormat};

    let tracks = render_tracks_individual(all_notes, cfg, soundfont, duration_sec)?;
    if tracks.is_empty() {
        return Err("Aucune piste à rendre".into());
    }

    let mut mix_l: Option<Vec<f32>> = None;
    let mut mix_r: Option<Vec<f32>> = None;

    for rt in &tracks {
        // Décoder et additionner au mix
        let Ok(mut rd) = WavReader::new(std::io::Cursor::new(&rt.wav)) else { continue; };
        let ch = rd.spec().channels as usize;
        let s: Vec<i16> = rd.samples::<i16>().filter_map(|x| x.ok()).collect();
        let n = s.len() / ch.max(1);
        if n == 0 { continue; }
        if mix_l.is_none() {
            mix_l = Some(vec![0f32; n]);
            mix_r = Some(vec![0f32; n]);
        }
        if let (Some(ml), Some(mr)) = (mix_l.as_mut(), mix_r.as_mut()) {
            let m = ml.len().min(n);
            for i in 0..m {
                ml[i] += s[i * ch] as f32 / 32768.0;
                mr[i] += if ch > 1 { s[i * ch + 1] as f32 / 32768.0 } else { s[i * ch] as f32 / 32768.0 };
            }
        }
    }

    let (Some(ml), Some(mr)) = (mix_l, mix_r) else {
        return Err("Aucune piste à rendre".into());
    };

    // Normalisation au pic (~0.5, -6 dBFS) → niveau stable quel que soit
    // le nombre de pistes, puis gain du master volume.
    let mut pic = 0f32;
    for i in 0..ml.len() {
        pic = pic.max(ml[i].abs()).max(mr[i].abs());
    }
    let norm = if pic > 1e-6 { 0.5 / pic } else { 1.0 };
    let gain = norm * (master_vol as f32) / 127.0;

    let spec = WavSpec {
        channels: 2, sample_rate: 44100,
        bits_per_sample: 16, sample_format: SampleFormat::Int,
    };
    let mut buf = Vec::new();
    if let Ok(mut w) = WavWriter::new(std::io::Cursor::new(&mut buf), spec) {
        for i in 0..ml.len() {
            let _ = w.write_sample(((ml[i] * gain).clamp(-1.0, 1.0) * 32767.0) as i16);
            let _ = w.write_sample(((mr[i] * gain).clamp(-1.0, 1.0) * 32767.0) as i16);
        }
        let _ = w.finalize();
    }
    if buf.is_empty() {
        return Err("Écriture du WAV impossible".into());
    }
    Ok(fade_out_wav(&buf, 30))
}

// ─── Piste de clic pour le rendu (mode Navig) ────────────────────────────

/// SMF de la piste de clic, rendu séparément puis mélangé au WAV principal →
/// synchronisation échantillon-parfaite par construction.
/// `sound` : 0 = métronome GM (percussion 33/34), 1 = Woodblock (115),
/// 2 = Agogo (114), 3 = Taiko (116). L'accent est sur le 1ᵉʳ temps de mesure.
pub fn generate_click_smf(tempo: u16, beats_per_bar: u64, total_beats: f64, accent: bool, sound: u8) -> Vec<u8> {
    let total = total_beats.ceil() as u64;
    let mut notes: Vec<CustomNote> = Vec::new();
    for b in 0..total {
        let acc = accent && b % beats_per_bar.max(1) == 0;
        let vel = if acc { 127 } else { 120 };
        // (canal, pitch, durée) selon le son choisi
        let (channel, pitch) = match sound {
            // Métronome GM : note 33 (clic) / 34 (cloche) sur le canal drums
            0 => (9u8, if acc { 34 } else { 33 }),
            // Sons mélodiques sur le canal 15 (program change ci-dessous)
            1 => (15u8, 72), // Woodblock
            2 => (15u8, 74), // Agogo
            _ => (15u8, 55), // Taiko
        };
        notes.push(CustomNote {
            channel,
            start_time: b as f64,
            pitch,
            duration: 0.15,
            velocity: vel,
        });
    }

    // Piste synthétique pour le program change des sons mélodiques
    let tracks: Vec<TrackCfg> = match sound {
        1 => vec![TrackCfg { channel: 15, program: 115, volume: 127, mute: false, drums: false, bank_msb: 0, bank_lsb: 0, fx: Default::default() }],
        2 => vec![TrackCfg { channel: 15, program: 114, volume: 127, mute: false, drums: false, bank_msb: 0, bank_lsb: 0, fx: Default::default() }],
        3 => vec![TrackCfg { channel: 15, program: 116, volume: 127, mute: false, drums: false, bank_msb: 0, bank_lsb: 0, fx: Default::default() }],
        _ => vec![], // métronome GM : kit drums natif, pas de program change
    };
    generate_smf_from_custom(&notes, &tracks, tempo)
}

/// Mélange deux WAV 16-bit (même fréquence d'échantillonnage) :
/// `main + click * click_gain`. Le click mono est dupliqué sur tous les
/// canaux du main ; la sortie est tronquée à la plus courte des deux.
pub fn mix_wavs(main: &[u8], click: &[u8], click_gain: f32) -> Result<Vec<u8>, String> {
    let mut mr = hound::WavReader::new(std::io::Cursor::new(main)).map_err(|e| e.to_string())?;
    let mut cr = hound::WavReader::new(std::io::Cursor::new(click)).map_err(|e| e.to_string())?;
    let spec = mr.spec();
    let mch = spec.channels.max(1) as usize;
    let cch = cr.spec().channels.max(1) as usize;
    let ms: Vec<i16> = mr.samples::<i16>().collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    let cs: Vec<i16> = cr.samples::<i16>().collect::<Result<_, _>>().map_err(|e| e.to_string())?;

    let nframes = (ms.len() / mch).min(cs.len() / cch);
    let mut out: Vec<i16> = Vec::with_capacity(nframes * mch);
    for f in 0..nframes {
        for ch in 0..mch {
            let m = ms[f * mch + ch];
            let c = cs[f * cch + (ch % cch)] as f32 * click_gain;
            out.push((m as f32 + c).clamp(-32768.0, 32767.0) as i16);
        }
    }

    let wspec = hound::WavSpec {
        channels: spec.channels,
        sample_rate: spec.sample_rate,
        bits_per_sample: spec.bits_per_sample,
        sample_format: hound::SampleFormat::Int,
    };
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut w = hound::WavWriter::new(std::io::Cursor::new(&mut buf), wspec)
            .map_err(|e| e.to_string())?;
        for s in out {
            w.write_sample(s).map_err(|e| e.to_string())?;
        }
        w.finalize().map_err(|e| e.to_string())?;
    }
    Ok(buf)
}

// ─── Custom notes (PianoRoll) ──────────────────────────────────────────

/// Données d'une note personnalisée (provenant du PianoRoll frontend).
#[derive(Clone, Debug, Serialize)]
pub struct CustomNote {
    pub channel: u8,
    pub start_time: f64,
    pub pitch: u8,
    pub duration: f64,
    pub velocity: u8,
}

/// Génère un SMF à partir de notes personnalisées (PianoRoll).
/// Remplace la génération automatique par accord.
pub fn generate_smf_from_custom(
    notes: &[CustomNote],
    tracks: &[TrackCfg],
    tempo: u16,
) -> Vec<u8> {
    let tpb = TICKS_PER_BEAT;
    let tempo_us = (60_000_000u64 / tempo.max(1) as u64) as u32;
    let mut evs: Vec<Ev> = Vec::new();

    // Tempo meta event
    e(&mut evs, 0, &[0xFF, 0x51, 0x03,
        ((tempo_us >> 16) & 0xFF) as u8,
        ((tempo_us >> 8)  & 0xFF) as u8,
        (tempo_us & 0xFF) as u8]);

    // Program changes pour chaque piste (le canal drums 9 est natif :
    // les pistes percussion sur d'autres canaux n'ont pas besoin de
    // programmation — leurs notes sont redirigées vers le canal 9).
    for tc in tracks {
        e(&mut evs, 0, &[0xC0 | tc.channel, tc.program as u8]);
    }

    // Appliquer les réglages de piste (mute) aux notes : une piste mutée
    // est silencieuse. Le scaling de vélocité par volume de piste est fait
    // en amont (render_wav pour les notes custom ; generate_notes pour les
    // notes classiques) — pas de double scaling ici.
    let find_track = |ch: u8| tracks.iter().find(|t| t.channel == ch);
    let mut audible: Vec<CustomNote> = notes.iter()
        .filter(|n| find_track(n.channel).map_or(true, |t| !t.mute))
        .cloned()
        .collect();

    // Pistes percussion (drums, canal ≠ 9) : leurs notes sont redirigées
    // vers le canal drums natif (9) — le kit sonne sur n'importe quelle
    // piste drums, quel que soit son canal de saisie.
    for n in audible.iter_mut() {
        if let Some(t) = find_track(n.channel) {
            if t.drums && n.channel != CH_DRUMS {
                n.channel = CH_DRUMS;
            }
        }
    }

    // Trier les notes par start_time
    audible.sort_by(|a, b| a.start_time.partial_cmp(&b.start_time).unwrap_or(std::cmp::Ordering::Equal));

    for n in &audible {
        let start_tick = (n.start_time * tpb as f64) as u32;
        let end_tick = ((n.start_time + n.duration) * tpb as f64) as u32;

        // Note On
        e(&mut evs, start_tick, &[0x90 | n.channel, n.pitch, n.velocity]);
        // Note Off
        e(&mut evs, end_tick, &[0x80 | n.channel, n.pitch, 64]);
    }

    // End of Track
    let last_tick = audible.last()
        .map(|n| ((n.start_time + n.duration) * tpb as f64 + 4.0) as u32)
        .unwrap_or(288);
    e(&mut evs, last_tick, &[0xFF, 0x2F, 0x00]);

    // Trier les événements par tick
    evs.sort_by(|a, b| a.tick.cmp(&b.tick));

    // ── Sérialisation SMF Format 0 ──
    let mut buf = Vec::new();
    // Header "MThd"
    buf.extend_from_slice(b"MThd");
    write_u32(&mut buf, 6);
    write_u16(&mut buf, 0); // Format 0
    write_u16(&mut buf, 1); // 1 track
    write_u16(&mut buf, tpb as u16);

    // Track chunk "MTrk"
    let mut track_data = Vec::new();
    let mut last_tick: u32 = 0;
    for ev in &evs {
        let delta = ev.tick.saturating_sub(last_tick);
        write_vlq(&mut track_data, delta);
        track_data.extend_from_slice(&ev.bytes);
        last_tick = ev.tick;
    }

    buf.extend_from_slice(b"MTrk");
    write_u32(&mut buf, track_data.len() as u32);
    buf.extend_from_slice(&track_data);

    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    /// apply_gain normalise au pic ~0,5 (-6 dBFS) puis applique le master —
    /// même règle que render_wav_mixed et render-tracks (niveaux unifiés).
    #[test]
    fn apply_gain_normalise_au_pic_puis_master() {
        let spec = hound::WavSpec {
            channels: 1, sample_rate: 44100, bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut buf = Vec::new();
        {
            let mut w = hound::WavWriter::new(std::io::Cursor::new(&mut buf), spec).unwrap();
            for i in 0..1000 {
                let v = if i == 500 { (0.25 * 32767.0) as i16 } else { 100 };
                w.write_sample(v).unwrap();
            }
            w.finalize().unwrap();
        }
        let out = apply_gain(&buf, 127);
        let mut r = hound::WavReader::new(std::io::Cursor::new(&out)).unwrap();
        let s: Vec<i16> = r.samples::<i16>().filter_map(|x| x.ok()).collect();
        let peak = s.iter().map(|&v| (v as f32).abs()).fold(0.0f32, f32::max);
        // pic cible = 0.5 × 32767 ≈ 16383 (±1 arrondi)
        assert!((peak - 16383.0).abs() <= 2.0, "peak {peak} != ~16383");
    }

    /// render_tracks_list ajoute une piste drums par défaut quand des notes
    /// canal 9 existent sans piste native (sinon elles seraient perdues).
    #[test]
    fn render_tracks_synthetise_la_piste_drums_manquante() {
        let cfg = RenderCfg {
            tempo: 120, pattern: "rock".into(), walking: false, sig: "4/4".into(), lead_inst: 51,
            tracks: vec![TrackCfg {
                channel: 5, program: 1, volume: 100, mute: false, drums: true,
                bank_msb: 0, bank_lsb: 0, fx: Default::default(),
            }],
        };
        let notes = vec![CustomNote {
            channel: CH_DRUMS, start_time: 0.0, pitch: 36, duration: 0.25, velocity: 100,
        }];
        let list = render_tracks_list(&cfg, &notes);
        assert!(list.iter().any(|t| t.channel == CH_DRUMS),
            "une piste drums canal 9 doit être synthétisée");
        // Sans notes canal 9 → pas de synthèse inutile
        let empty: Vec<CustomNote> = vec![];
        let list2 = render_tracks_list(&cfg, &empty);
        assert!(!list2.iter().any(|t| t.channel == CH_DRUMS),
            "pas de synthèse sans notes canal 9");
        // Piste native canal 9 présente → rien d'ajouté
        let cfg2 = RenderCfg {
            tempo: 120, pattern: "rock".into(), walking: false, sig: "4/4".into(), lead_inst: 51,
            tracks: vec![
                TrackCfg {
                    channel: 5, program: 1, volume: 100, mute: false, drums: true,
                    bank_msb: 0, bank_lsb: 0, fx: Default::default(),
                },
                TrackCfg {
                    channel: 9, program: 1, volume: 100, mute: false, drums: false,
                    bank_msb: 0, bank_lsb: 0, fx: Default::default(),
                },
            ],
        };
        let list3 = render_tracks_list(&cfg2, &notes);
        assert_eq!(list3.iter().filter(|t| t.channel == CH_DRUMS).count(), 1);
    }

    /// Les noms de fichiers temporaires de rendu doivent être UNIQUES par
    /// appel : deux rendus simultanés (ex. scrubs rapides en mode séparé)
    /// partageaient /tmp/chordj_render.mid|.wav → l'un supprimait les
    /// fichiers pendant que l'autre les lisait (« No such file or directory »).
    #[test]
    fn render_temp_tag_est_unique() {
        let a = next_render_tag();
        let b = next_render_tag();
        assert_ne!(a, b, "deux tags consécutifs doivent différer");
        let c = next_render_tag();
        assert_ne!(b, c);
        // Le tag contient le PID et un numéro → fichiers distincts
        assert!(a.starts_with("chordj_render_"));
    }

    /// slice_wav_from retire le début d'un WAV (offset en secondes) —
    /// utilisé par le repli mixé de navig-play pour démarrer à la position.
    #[test]
    fn slice_wav_from_coupe_le_debut() {
        let spec = hound::WavSpec {
            channels: 1, sample_rate: 1000, bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut buf = Vec::new();
        {
            let mut w = hound::WavWriter::new(std::io::Cursor::new(&mut buf), spec).unwrap();
            for i in 0..1000 {
                w.write_sample(i as i16).unwrap();
            }
            w.finalize().unwrap();
        }
        // offset 0 → identique
        assert_eq!(slice_wav_from(&buf, 0.0).len(), buf.len());
        // offset 0,25 s (250 éch @1000 Hz) → 750 échantillons restants
        let sliced = slice_wav_from(&buf, 0.25);
        let mut r = hound::WavReader::new(std::io::Cursor::new(&sliced)).unwrap();
        let s: Vec<i16> = r.samples::<i16>().filter_map(|x| x.ok()).collect();
        assert_eq!(s.len(), 750, "250 échantillons retirés");
        assert_eq!(s[0], 250, "le premier échantillon = index 250 d'origine");
        assert_eq!(s[749], 999, "le dernier échantillon = index 999 d'origine");
        // offset au-delà de la durée → inchangé (garde-fou)
        assert_eq!(slice_wav_from(&buf, 99.0).len(), buf.len());
    }

    // ── SMF du clic métronome (feature récente : clic temps réel / rendu) ──

    /// Parse un SMF et renvoie les note-on : (canal, pitch, vélocité).
    /// Mini-parseur suffisant pour les fichiers générés par le serveur
    /// (gère delta-time VLQ, running status, meta events).
    fn parse_smf_notes(smf: &[u8]) -> Vec<(u8, u8, u8)> {
        let mut out = Vec::new();
        let mut i = 0;
        assert_eq!(&smf[i..i + 4], b"MThd", "header SMF attendu");
        i += 4;
        let hlen = u32::from_be_bytes(smf[i..i + 4].try_into().unwrap()) as usize;
        i += 4 + hlen;
        while i + 4 <= smf.len() {
            assert_eq!(&smf[i..i + 4], b"MTrk", "piste SMF attendue");
            i += 4;
            let tlen = u32::from_be_bytes(smf[i..i + 4].try_into().unwrap()) as usize;
            i += 4;
            let end = i + tlen;
            let mut running: Option<u8> = None;
            while i < end {
                // delta-time (VLQ)
                loop {
                    let b = smf[i];
                    i += 1;
                    if b & 0x80 == 0 {
                        break;
                    }
                }
                let mut status = smf[i];
                if status & 0x80 == 0 {
                    status = running.expect("running status sans statut initial");
                } else {
                    i += 1;
                }
                match status {
                    0x90..=0x9f | 0x80..=0x8f => {
                        let d1 = smf[i];
                        let d2 = smf[i + 1];
                        i += 2;
                        running = Some(status);
                        if status & 0xf0 == 0x90 && d2 > 0 {
                            out.push((status & 0x0f, d1, d2));
                        }
                    }
                    0xc0..=0xcf | 0xd0..=0xdf => {
                        i += 1;
                        running = Some(status);
                    }
                    0xb0..=0xbf | 0xe0..=0xef => {
                        i += 2;
                        running = Some(status);
                    }
                    0xff => {
                        i += 1; // type de meta event
                        let mut mlen = 0u32;
                        loop {
                            let b = smf[i];
                            i += 1;
                            mlen = (mlen << 7) | (b & 0x7f) as u32;
                            if b & 0x80 == 0 {
                                break;
                            }
                        }
                        i += mlen as usize; // données du meta event
                        running = None;
                    }
                    _ => {
                        running = Some(status);
                    }
                }
            }
            i = end;
        }
        out
    }

    /// generate_click_smf : une note par battement, accent (vel 127) sur
    /// chaque début de mesure — exactement le comportement du métronome
    /// rendu dans le WAV.
    #[test]
    fn generate_click_smf_notes_et_accents() {
        // 4/4, 8 battements → 8 notes, accents aux beats 0 et 4
        let smf = generate_click_smf(120, 4, 8.0, true, 0);
        assert!(smf.starts_with(b"MThd"));
        let evs = parse_smf_notes(&smf);
        assert_eq!(evs.len(), 8);
        let accented = evs.iter().filter(|(_, _, v)| *v == 127).count();
        assert_eq!(accented, 2, "2 débuts de mesure sur 8 battements en 4/4");
        // Sans accent : aucune vélocité 127
        let smf = generate_click_smf(120, 4, 8.0, false, 0);
        let evs = parse_smf_notes(&smf);
        assert_eq!(evs.len(), 8);
        assert!(evs.iter().all(|(_, _, v)| *v == 120));
        // 7/8 : accent au beat 0 seulement (mesures de 7 battements)
        let smf = generate_click_smf(120, 7, 14.0, true, 0);
        let evs = parse_smf_notes(&smf);
        assert_eq!(evs.len(), 14);
        let accented = evs.iter().filter(|(_, _, v)| *v == 127).count();
        assert_eq!(accented, 2);
    }

    /// generate_click_smf : canal/pitch selon le son choisi (métronome GM
    /// sur le canal drums, woodblock/agogo/taiko mélodiques sur le 15).
    #[test]
    fn generate_click_smf_sons_et_pitches() {
        // Métronome GM : canal 9, cloche 34 (accent) / clic 33 (normal)
        let smf = generate_click_smf(120, 4, 4.0, true, 0);
        let evs = parse_smf_notes(&smf);
        assert_eq!(evs.len(), 4);
        assert_eq!(evs[0], (9, 34, 127));
        assert_eq!(evs[1], (9, 33, 120));
        // Woodblock : canal 15, pitch 72
        let smf = generate_click_smf(120, 4, 1.0, true, 1);
        let evs = parse_smf_notes(&smf);
        assert_eq!(evs[0], (15, 72, 127));
        // Taiko : canal 15, pitch 55
        let smf = generate_click_smf(120, 4, 1.0, true, 3);
        let evs = parse_smf_notes(&smf);
        assert_eq!(evs[0], (15, 55, 127));
    }

    // ── Mix main + clic (mode « clic dans le rendu ») ──

    /// mix_wavs : somme main + clic×gain, clamp 16-bit, troncature à la
    /// plus courte des deux.
    #[test]
    fn mix_wavs_melange_et_clamp() {
        let spec = hound::WavSpec {
            channels: 1, sample_rate: 44100, bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut main: Vec<u8> = Vec::new();
        {
            let mut w = hound::WavWriter::new(std::io::Cursor::new(&mut main), spec).unwrap();
            for v in [100i16, 32000, -32000, 0] {
                w.write_sample(v).unwrap();
            }
            w.finalize().unwrap();
        }
        let mut click: Vec<u8> = Vec::new();
        {
            let mut w = hound::WavWriter::new(std::io::Cursor::new(&mut click), spec).unwrap();
            for v in [100i16, 32000, -32000, 0] {
                w.write_sample(v).unwrap();
            }
            w.finalize().unwrap();
        }
        let out = mix_wavs(&main, &click, 0.5).unwrap();
        let mut r = hound::WavReader::new(std::io::Cursor::new(&out)).unwrap();
        let s: Vec<i16> = r.samples::<i16>().filter_map(|x| x.ok()).collect();
        assert_eq!(s.len(), 4);
        assert_eq!(s[0], 150); // 100 + 100×0,5
        assert_eq!(s[1], 32767); // clamp haut (32000 + 16000)
        assert_eq!(s[2], -32768); // clamp bas (−32000 − 16000)
        assert_eq!(s[3], 0);
        // Gain 0 → identique au main
        let out = mix_wavs(&main, &click, 0.0).unwrap();
        let mut r = hound::WavReader::new(std::io::Cursor::new(&out)).unwrap();
        let s: Vec<i16> = r.samples::<i16>().filter_map(|x| x.ok()).collect();
        assert_eq!(s[0], 100);
    }
}

mod tests_courtes {
    use super::*;

    fn cfg_test() -> RenderCfg {
        RenderCfg {
            tempo: 120, pattern: "rock".into(), walking: true, sig: "4/4".into(), lead_inst: 51,
            tracks: vec![
                TrackCfg { channel: CH_LEAD, program: 1, volume: 100, mute: false, drums: false, bank_msb: 0, bank_lsb: 0, fx: Default::default() },
                TrackCfg { channel: CH_BASS, program: 1, volume: 100, mute: false, drums: false, bank_msb: 0, bank_lsb: 0, fx: Default::default() },
                TrackCfg { channel: CH_STR, program: 1, volume: 100, mute: false, drums: false, bank_msb: 0, bank_lsb: 0, fx: Default::default() },
                TrackCfg { channel: CH_DRUMS, program: 1, volume: 100, mute: false, drums: true, bank_msb: 0, bank_lsb: 0, fx: Default::default() },
                TrackCfg { channel: CH_ACC, program: 1, volume: 100, mute: false, drums: false, bank_msb: 0, bank_lsb: 0, fx: Default::default() },
            ],
        }
    }

    /// Notation 8: (croche = 0,5 temps) : lead plaqué, basse tenue bc, drums
    /// au temps 0 — et AUCUNE note ne déborde de la durée de l'accord.
    #[test]
    fn accord_8_croche_complet() {
        let cfg = cfg_test();
        let notes_arrays = vec![vec![48, 60, 64, 67]]; // basse C2 + C E G
        let out = generate_notes(&notes_arrays, &[0.5], &cfg);

        assert!(out.iter().any(|n| n.channel == CH_LEAD), "lead doit jouer sur une croche");
        assert!(out.iter().any(|n| n.channel == CH_DRUMS), "drums doivent jouer sur une croche");
        let basses: Vec<_> = out.iter().filter(|n| n.channel == CH_BASS).collect();
        assert_eq!(basses.len(), 1, "une seule note de basse");
        assert!((basses[0].duration - 0.5).abs() < 1e-9, "basse tenue sur toute la croche");
        assert!(out.iter().all(|n| n.start_time + n.duration <= 0.5 + 1e-9),
            "aucune note ne doit déborder de l'accord (8:)");
    }

    /// Notation 6: (triolet de noire ≈ 0,667 temps) : idem, sans débordement.
    #[test]
    fn accord_6_triolet_noire_complet() {
        let cfg = cfg_test();
        let notes_arrays = vec![vec![48, 60, 64, 67]];
        let out = generate_notes(&notes_arrays, &[2.0 / 3.0], &cfg);

        assert!(out.iter().any(|n| n.channel == CH_LEAD), "lead doit jouer");
        assert!(out.iter().any(|n| n.channel == CH_DRUMS), "drums doivent jouer");
        let basses: Vec<_> = out.iter().filter(|n| n.channel == CH_BASS).collect();
        assert_eq!(basses.len(), 1);
        assert!((basses[0].duration - 2.0 / 3.0).abs() < 1e-9);
        assert!(out.iter().all(|n| n.start_time + n.duration <= 2.0 / 3.0 + 1e-9),
            "aucune note ne doit déborder de l'accord (6:)");
    }

    /// Notation 12: (triolet de croche = 0,333 temps) : idem.
    #[test]
    fn accord_12_triolet_croche_complet() {
        let cfg = cfg_test();
        let notes_arrays = vec![vec![48, 60, 64, 67]];
        let out = generate_notes(&notes_arrays, &[1.0 / 3.0], &cfg);

        assert!(out.iter().any(|n| n.channel == CH_LEAD), "lead doit jouer");
        assert!(out.iter().any(|n| n.channel == CH_DRUMS), "drums doivent jouer");
        assert!(out.iter().all(|n| n.start_time + n.duration <= 1.0 / 3.0 + 1e-9),
            "aucune note ne doit déborder de l'accord (12:)");
    }

    /// Notation 3: (1,333 temps) : le lead skank joue (1 attaque), drums au
    /// temps 0, et le lead ne déborde pas (le contretemps 0,5 est dans l'accord).
    #[test]
    fn accord_3_tiers_de_mesure_lead_et_drums() {
        let cfg = cfg_test();
        let notes_arrays = vec![vec![48, 60, 64, 67]];
        let out = generate_notes(&notes_arrays, &[4.0 / 3.0], &cfg);

        let leads: Vec<_> = out.iter().filter(|n| n.channel == CH_LEAD).collect();
        assert!(!leads.is_empty(), "lead doit jouer");
        assert!(leads.iter().all(|n| n.start_time + n.duration <= 4.0 / 3.0 + 1e-9),
            "le lead ne doit pas déborder de l'accord (3:)");
        assert!(out.iter().any(|n| n.channel == CH_DRUMS), "drums doivent jouer");
    }
}
