/// Render chord progression — génération SMF Format 0 + render WAV.
use std::process::Command;
use crate::patterns::sc;
use crate::walking::{is_minor as is_minor_chord, generate_walking_bass as walking_bass_notes};

const TICKS_PER_BEAT: u32 = 480;

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

fn write_u16(buf: &mut Vec<u8>, v: u16) { buf.extend_from_slice(&v.to_be_bytes()); }
fn write_u32(buf: &mut Vec<u8>, v: u32) { buf.extend_from_slice(&v.to_be_bytes()); }

const CH_LEAD: u8 = 0; const CH_BASS: u8 = 2;
const CH_STR: u8 = 3;  const CH_ACC: u8 = 4;  const CH_DRUMS: u8 = 9;
const KICK: u8 = 36; const SNARE: u8 = 38; const HH: u8 = 42; const RIM: u8 = 37;

// ─── Config de rendu ─────────────────────────────────────────────────
pub struct RenderCfg {
    pub tempo: u32,
    pub pattern: String,     // rock|reggae|jazz|pop|bossa|onedrop
    pub walking: bool,
    pub sig: String,          // "4/4"
    pub lead_inst: u16,
    pub tracks: [TrackCfg; 5],
}

#[derive(Clone, Copy)]
pub struct TrackCfg {
    pub channel: u8,
    pub program: u16,
    pub volume: u8,
    pub mute: bool,
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
struct Ev { tick: u32, bytes: Vec<u8> }

fn e(evs: &mut Vec<Ev>, tick: u32, bytes: &[u8]) {
    evs.push(Ev { tick, bytes: bytes.to_vec() });
}

pub fn generate_smf_fmt0(
    notes_arrays: &[Vec<u8>],
    beats: &[f64],
    cfg: &RenderCfg,
) -> Vec<u8> {
    let tempo_us = (60_000_000u64 / cfg.tempo.max(1) as u64) as u32;
    let tpb = TICKS_PER_BEAT;
    let eighth = tpb / 2;

    // Résoudre la signature
    let sig_parts: Vec<&str> = cfg.sig.split('/').collect();
    let beats_per_bar = sig_parts.first().and_then(|s| s.parse::<u32>().ok()).unwrap_or(4);

    let mut evs: Vec<Ev> = Vec::new();

    // Tempo
    e(&mut evs, 0, &[0xFF, 0x51, 0x03,
        ((tempo_us >> 16) & 0xFF) as u8,
        ((tempo_us >> 8)  & 0xFF) as u8,
        (tempo_us & 0xFF) as u8]);

    // Init PCs depuis la config tracks (drums: le program selectionne le kit)
    for tc in &cfg.tracks {
        e(&mut evs, 0, &[0xC0 | tc.channel, tc.program as u8]);
    }
    // Si lead channel n'est pas dans tracks, utiliser cfg.lead_inst
    let has_lead = cfg.tracks.iter().any(|t| t.channel == CH_LEAD);
    if !has_lead {
        e(&mut evs, 0, &[0xC0 | CH_LEAD, cfg.lead_inst as u8]);
    }

    // Extraire les configs par canal
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

    // Boucle accords
    let mut abs_tick = 0u32;
    let mut seed: u64 = 0;

    for (ci, notes) in notes_arrays.iter().enumerate() {
        let bc = if ci < beats.len() { beats[ci] } else { 4.0 };
        let total_ticks = (bc * tpb as f64) as u32;
        let nq = bc as u32;

        if notes.is_empty() { abs_tick += total_ticks; continue; }

        let chord_start = abs_tick;
        let chord_end = abs_tick + total_ticks;
        abs_tick = chord_end;

        let bass_note = notes[0];
        let chord: &[u8] = if notes.len() > 1 { &notes[1..] } else { &[] };

        // Lead — pompe skank : staccato sur contretemps 8eme
        // Note On au tick = beat * tpb + 240, Off 120 ticks plus tard (staccato 16eme)
        if !lead_mute {
            let lv = sc(lead_vol, 127);
            for b in 0..nq {
                let skank_on = chord_start + b * tpb + eighth; // 8eme offbeat = tick 240
                for &n in chord { e(&mut evs, skank_on, &[0x90, n, lv]); }
                let skank_off = skank_on + 120; // staccato 1/16eme
                for &n in chord { e(&mut evs, skank_off, &[0x80, n, 64]); }
            }
        }

        // Walking bass ou note tenue
        if !bass_mute {
            let bv = sc(bass_vol, 127);
            if cfg.walking && notes.len() >= 2 {
                let next_root = notes_arrays.get(ci + 1).and_then(|n| n.first()).copied()
                    .or_else(|| notes_arrays.first().and_then(|n| n.first()).copied())
                    .unwrap_or(bass_note);
                let wb_notes = walking_bass_notes(&[bass_note, chord[0], chord.get(1).copied().unwrap_or(bass_note)], next_root, seed, is_minor_chord(&[bass_note, chord[0]]));
                seed = seed.wrapping_add(1);
                for (bi, &bn) in wb_notes.iter().enumerate() {
                    let bt = chord_start + (bi as u32) * tpb; // toutes les noires
                    e(&mut evs, bt, &[0x92, bn, bv]);
                    // Note Off 1 tick avant la note suivante (staccato)
                    let off_tick = if bi < 3 { chord_start + ((bi + 1) as u32) * tpb - 1 } else { chord_end };
                    e(&mut evs, off_tick, &[0x82, bn, 64]);
                }
            } else {
                e(&mut evs, chord_start, &[0x92, bass_note, bv]);
                e(&mut evs, chord_end, &[0x82, bass_note, 64]);
            }
        }

        if !str_mute {
            let sv = sc(str_vol, 127);
            for &n in chord { e(&mut evs, chord_start, &[0x93, n, sv]); }
            for &n in chord { e(&mut evs, chord_end, &[0x83, n, 64]); }
        }

        // Drums + Accent
        for b in 0..nq {
            let on_tick = chord_start + b * tpb;
            let up_tick = on_tick + eighth;
            let bar_beat = (abs_tick / tpb + b) % beats_per_bar;

            // Druns — pattern exactement comme drum_hit() dans main.rs
            if !drums_mute {
                let dv = sc(drums_vol, 127);
                let hh = |b:u8| sc(dv, b); let hh_beat = hh(80); let hh_eighth = hh(65);
                let hh55 = hh(55); let hh45 = hh(45); let hh40 = hh(10);
                let hh60 = hh(60); let hh65 = hh(65);
                let kk = |b:u8| sc(dv, b); let sn = |b:u8| sc(dv, b);
                
                match cfg.pattern.as_str() {
                    "reggae" => {
                        match bar_beat { // on-beat only
                            0|1|3 => { e(&mut evs, on_tick, &[0x99, HH, hh60]); }
                            2 => { e(&mut evs, on_tick, &[0x99, KICK, kk(120)]); e(&mut evs, on_tick, &[0x99, HH, hh65]); e(&mut evs, on_tick, &[0x99, RIM, kk(90)]); }
                            _ => {}
                        }
                        e(&mut evs, up_tick, &[0x99, HH, hh40]);
                    }
                    "jazz" => {
                        let bb2 = bar_beat % 8; // 2 mesures
                        match bb2 {
                            0|2|6 => { e(&mut evs, on_tick, &[0x99, 51, hh60]); }
                            4 => { e(&mut evs, on_tick, &[0x99, 51, hh60]); e(&mut evs, on_tick, &[0x99, 44, hh(40)]); }
                            7 => { e(&mut evs, on_tick, &[0x99, 51, hh60]); e(&mut evs, on_tick, &[0x99, 44, hh(40)]); e(&mut evs, on_tick, &[0x99, RIM, hh(50)]); }
                            _ => {}
                        }
                        e(&mut evs, up_tick, &[0x99, HH, 35]);
                    }
                    "pop" => {
                        match bar_beat % 4 {
                            0 => { e(&mut evs, on_tick, &[0x99, KICK, kk(85)]); e(&mut evs, on_tick, &[0x99, HH, hh(50)]); }
                            1 => { e(&mut evs, on_tick, &[0x99, SNARE, sn(70)]); e(&mut evs, on_tick, &[0x99, HH, hh(50)]); }
                            2 => { e(&mut evs, on_tick, &[0x99, KICK, kk(75)]); }
                            3 => { e(&mut evs, on_tick, &[0x99, SNARE, sn(65)]); e(&mut evs, on_tick, &[0x99, HH, hh(50)]); }
                            _ => {}
                        }
                        e(&mut evs, up_tick, &[0x99, HH, hh(45)]);
                    }
                    "bossa" => {
                        match bar_beat % 4 {
                            0 => { e(&mut evs, on_tick, &[0x99, KICK, kk(55)]); e(&mut evs, on_tick, &[0x99, HH, hh45]); }
                            1 => { e(&mut evs, on_tick, &[0x99, SNARE, sn(30)]); e(&mut evs, on_tick, &[0x99, HH, hh45]); }
                            2 => { e(&mut evs, on_tick, &[0x99, KICK, kk(60)]); e(&mut evs, on_tick, &[0x99, HH, hh45]); }
                            3 => { e(&mut evs, on_tick, &[0x99, KICK, kk(50)]); e(&mut evs, on_tick, &[0x99, HH, hh45]); }
                            _ => {}
                        }
                        e(&mut evs, up_tick, &[0x99, HH, hh40]);
                    }
                    "onedrop" => {
                        match bar_beat % 4 {
                            0 => { e(&mut evs, on_tick, &[0x99, KICK, kk(90)]); e(&mut evs, on_tick, &[0x99, HH, hh55]); }
                            1 => { e(&mut evs, on_tick, &[0x99, HH, hh40]); }
                            2 => { e(&mut evs, on_tick, &[0x99, KICK, kk(90)]); e(&mut evs, on_tick, &[0x99, RIM, sn(65)]); e(&mut evs, on_tick, &[0x99, HH, hh45]); }
                            3 => { e(&mut evs, on_tick, &[0x99, HH, hh55]); }
                            _ => {}
                        }
                        e(&mut evs, up_tick, &[0x99, HH, hh40]);
                    }
                    _ => { // rock (default) — HH sur chaque temps, rimshot seulement si specifique
                        match bar_beat % 4 {
                            0 => { e(&mut evs, on_tick, &[0x99, KICK, kk(90)]); e(&mut evs, on_tick, &[0x99, HH, hh_beat]); }
                            1 => { e(&mut evs, on_tick, &[0x99, SNARE, sn(75)]); e(&mut evs, on_tick, &[0x99, HH, hh_beat]); }
                            2 => { e(&mut evs, on_tick, &[0x99, KICK, kk(80)]); e(&mut evs, on_tick, &[0x99, HH, hh_beat]); }
                            3 => { e(&mut evs, on_tick, &[0x99, SNARE, sn(70)]); e(&mut evs, on_tick, &[0x99, HH, hh_beat]); }
                            _ => {}
                        }
                        e(&mut evs, up_tick, &[0x99, HH, hh_eighth]);
                    }
                }
            }

            // Accent (temps 2&4 — seulement si pas mute)
            if !acc_mute && (b == 1 || b == 3) {
                let av = sc(acc_vol, 127);
                for &n in chord {
                    e(&mut evs, on_tick, &[0x94, n, av]);
                    e(&mut evs, on_tick + 1, &[0x84, n, 64]);
                }
            }
        }

        // Note Offs (lead + strings; walking bass gère ses propres note-offs)
        if !lead_mute {
            let _lv = sc(lead_vol, 127);
            for &n in chord { e(&mut evs, chord_end, &[0x80, n, 64]); }
        }
        if !str_mute {
            for &n in chord { e(&mut evs, chord_end, &[0x83, n, 64]); }
        }
        if !bass_mute && !cfg.walking && chord.len() >= 1 {
            // Note Off déjà géré dans le walking ou la tenue
        }
    }

    // Sérialisation
    evs.sort_by_key(|e| e.tick);
    let mut t = Vec::new();
    let mut prev_tick = 0u32;
    for ev in &evs {
        let delta = ev.tick - prev_tick;
        write_vlq(&mut t, delta);
        t.extend_from_slice(&ev.bytes);
        prev_tick = ev.tick;
    }
    write_vlq(&mut t, 0);
    t.extend_from_slice(&[0xFF, 0x2F, 0x00]);

    let mut smf = Vec::new();
    smf.extend_from_slice(b"MThd");
    write_u32(&mut smf, 6); write_u16(&mut smf, 0); write_u16(&mut smf, 1); write_u16(&mut smf, tpb as u16);
    smf.extend_from_slice(b"MTrk"); write_u32(&mut smf, t.len() as u32); smf.extend_from_slice(&t);
    smf
}

// ─── Render WAV ──────────────────────────────────────────────────────
fn trim_to_duration(wav: &[u8], expected_sec: f64) -> Vec<u8> {
    use hound::WavReader;
    let Ok(mut reader) = WavReader::new(std::io::Cursor::new(wav)) else { return wav.to_vec(); };
    let spec = reader.spec();
    let samples: Vec<i16> = reader.samples::<i16>().filter_map(|s| s.ok()).collect();
    if samples.is_empty() { return wav.to_vec(); }
    let expected_samples = (expected_sec * spec.sample_rate as f64).round() as usize * spec.channels as usize;
    let end = expected_samples.min(samples.len());
    let mut out = Vec::new();
    if let Ok(mut w) = hound::WavWriter::new(std::io::Cursor::new(&mut out), spec) {
        for &s in &samples[..end] { let _ = w.write_sample(s); }
        let _ = w.finalize();
    }
    if out.is_empty() { wav.to_vec() } else { out }
}

pub fn render_wav(smf: &[u8], soundfont: &str, duration_sec: f64) -> Result<Vec<u8>, String> {
    let mid_path = std::env::temp_dir().join("chordj_render.mid");
    let wav_path = std::env::temp_dir().join("chordj_render.wav");
    std::fs::write(&mid_path, smf).map_err(|e| format!("write mid: {e}"))?;
    let output = Command::new("fluidsynth")
        .arg("-F").arg(&wav_path).arg("-T").arg("wav")
        .arg("-g").arg("1.0").arg("-n").arg("-i")
        .arg(soundfont).arg(&mid_path)
        .output().map_err(|e| format!("fluidsynth exec: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = std::fs::remove_file(&mid_path);
        return Err(format!("fluidsynth failed: {stderr}"));
    }
    if let Ok(s) = String::from_utf8(output.stderr) {
        let _ = std::fs::write(std::env::temp_dir().join("chordj_render.log"), &s);
    }
    let wav = std::fs::read(&wav_path).map_err(|e| format!("read wav: {e}"))?;
    let _ = std::fs::remove_file(&mid_path);
    let _ = std::fs::remove_file(&wav_path);
    Ok(trim_to_duration(&wav, duration_sec))
}
