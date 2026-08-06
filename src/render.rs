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
    pub fx: crate::dsp::Fx,  // Effets par piste (reverb/chorus/delay/drive)
}

impl Default for RenderCfg {
    fn default() -> Self {
        Self {
            tempo: 120, pattern: "rock".into(), walking: false, sig: "4/4".into(), lead_inst: 51,
            tracks: vec![
                TrackCfg { channel: 0, program: 51, volume: 60, mute: false, drums: false, fx: Default::default() },
                TrackCfg { channel: 2, program: 33, volume: 70, mute: false, drums: false, fx: Default::default() },
                TrackCfg { channel: 3, program: 48, volume: 60, mute: false, drums: false, fx: Default::default() },
                TrackCfg { channel: 9, program: 1, volume: 90, mute: false, drums: false, fx: Default::default() },
                TrackCfg { channel: 4, program: 2, volume: 50, mute: false, drums: false, fx: Default::default() },
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
        // Start = beat + 0.5 (contretemps), durée 0.25 (1/16 staccato)
        if !lead_mute {
            let lv = sc(lead_vol, 127);
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
                let short = (TICKS_PER_BEAT as f64 - 1.0) / tpb;
                for (bi, &bn) in wb_notes.iter().enumerate() {
                    let dur = if bi < 3 { short } else { bc - 3.0 };
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
        for b in 0..nq {
            // Beat absolu pour le pattern (wrap à beats_per_bar)
            let bar_beat = (abs_beats as u32 + b) % beats_per_bar;
            let t0 = abs_beats + b as f64;   // Début du temps
            let up = t0 + 0.5;               // Contretemps 8ème

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
                        hit(HH, hh40, up);
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
                        hit(HH, 35, up);
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
                        hit(HH, sc(dv, 45), up);
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
                        hit(HH, hh40, up);
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
                        hit(HH, hh40, up);
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
                        hit(HH, hh_eighth, up);
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

/// Normalise un WAV au pic cible (-1 dBFS) puis applique le master volume.
///
/// Le rendu FluidSynth utilise les vélocités des pistes (souvent faibles,
/// ex: lead 15/127) → le WAV brut sort très bas. Cette fonction amplifie
/// le WAV pour que le pic atteigne ~0.89×32768 (-1 dB), puis scale par
/// `master_vol/127`. La normalisation au pic garantit l'absence de
/// clipping (le gain ne dépasse jamais le ratio pic-cible).
/// Applique le volume master comme gain linéaire (pas de normalisation au pic).
/// Le gain Fluidsynth -g 0.5 + les vélocités modérées donnent un mix brut
/// doux (pic ~0.14) ; on compense par un gain fixe ×3 (niveau plein ~-4 dBFS
/// sans écraser les différences de volume entre pistes ni les mutes).
fn apply_gain(wav: &[u8], master_vol: u8) -> Vec<u8> {
    use hound::WavReader;
    let Ok(mut reader) = WavReader::new(std::io::Cursor::new(wav)) else {
        return wav.to_vec();
    };
    let spec = reader.spec();
    let samples: Vec<i16> = reader.samples::<i16>().filter_map(|s| s.ok()).collect();
    if samples.is_empty() { return wav.to_vec(); }

    let master = (master_vol as f64 / 127.0).clamp(0.0, 1.0);
    let gain = master * 3.0;

    let mut out = Vec::new();
    if let Ok(mut w) = hound::WavWriter::new(std::io::Cursor::new(&mut out), spec) {
        for &s in &samples {
            let v = (s as f64 * gain).round().clamp(-32768.0, 32767.0) as i16;
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
/// 1. Écrit le SMF dans `/tmp/chordj_render.mid`
/// 2. Appelle `fluidsynth -F <wav> -T wav -g 1.0 -n -i <soundfont> <mid>`
/// 3. Lit le WAV produit
/// 4. Nettoie les fichiers temporaires
/// 5. Tronque le WAV à la durée exacte
pub fn render_wav_raw(smf: &[u8], soundfont: &str, duration_sec: f64) -> Result<Vec<u8>, String> {
    let mid_path = std::env::temp_dir().join("chordj_render.mid");
    let wav_path = std::env::temp_dir().join("chordj_render.wav");

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
    for t in cfg.tracks.iter().filter(|t| !t.mute) {
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
