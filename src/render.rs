/// Render chord progression — génération SMF Format 0 + render WAV.
use std::process::Command;

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

// ─── Walking Bass ─────────────────────────────────────────────────────
const MIN_NOTE:u8=22; const MAX_NOTE:u8=42;

fn bass_clamp(n: u8) -> u8 {
    if n < MIN_NOTE { n + 12 } else if n > MAX_NOTE { n - 12 } else { n }
}

fn is_minor_chord(chord: &[u8]) -> bool {
    if chord.len() < 2 { return false; }
    let root = chord[0];
    let mut has_minor = false;
    let mut has_major = false;
    for &n in chord {
        let interval = if n >= root { n - root } else { n + 12 - root };
        match interval { 3 => has_minor = true, 4 => has_major = true, _ => {} }
    }
    has_minor && !has_major
}

fn walking_bass_notes(current: &[u8], next_root: u8, seed: u64, minor: bool) -> [u8; 4] {
    let root = current[0];
    let tones: Vec<u8> = if current.len() > 1 {
        let mut v: Vec<u8> = current[1..].iter().map(|&n| bass_clamp(n)).collect();
        v.sort(); v.dedup(); v
    } else { vec![root.saturating_sub(5)] };

    let b1 = root;
    let b2 = if minor {
        match seed % 100 {
            0..=24 => root + 2, 25..=49 => root.wrapping_sub(10),
            _ => tones[seed as usize % tones.len()],
        }
    } else { tones[seed as usize % tones.len()] };

    let filtered: Vec<u8> = tones.iter().filter(|&&n| n != b2).copied().collect();
    let b3 = if filtered.is_empty() { b2 + 7 } else { filtered[(seed.wrapping_add(7) as usize) % filtered.len()] };

    let b4 = match (seed % 100) as u8 {
        0..=49 => { let dir = if seed % 2 == 0 { 1i8 } else { -1i8 };
            let mut app = (next_root as i16 + dir as i16) as u8;
            if app < MIN_NOTE { app = next_root + 12; } if app > MAX_NOTE { app = next_root - 12; } app }
        50..=67 => { let mut app = next_root + 7; if app > MAX_NOTE { app -= 12; } app }
        68..=85 => { let mut app = next_root.wrapping_sub(5); if app < MIN_NOTE { app += 12; } app }
        _ => tones.iter().min_by_key(|&&t| { let d = if t > next_root { t-next_root } else { next_root-t }; d }).copied().unwrap_or(next_root),
    };
    [b1, b2, b3, b4]
}

// ─── SMF Format 0 ─────────────────────────────────────────────────────
struct Ev { tick: u32, bytes: Vec<u8> }

fn e(evs: &mut Vec<Ev>, tick: u32, bytes: &[u8]) {
    evs.push(Ev { tick, bytes: bytes.to_vec() });
}

// volume scaling helper
fn sc(vol: u8, base: u8) -> u8 { ((vol as u16 * base as u16) / 127).min(127) as u8 }

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

    // Init PCs depuis la config tracks
    for tc in &cfg.tracks {
        let ch = tc.channel;
        if ch == CH_DRUMS { e(&mut evs, 0, &[0xC0 | ch, 0]); } // GM drum bank = prog 0
        else { e(&mut evs, 0, &[0xC0 | ch, tc.program as u8]); }
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

        if !lead_mute {
            let lv = sc(lead_vol, 127);
            for &n in chord { e(&mut evs, chord_start, &[0x90, n, lv]); }
        }

        // Walking bass ou note tenue
        if !bass_mute {
            let bv = sc(bass_vol, 127);
            if cfg.walking && chord.len() >= 2 {
                let next_root = notes_arrays.get(ci + 1).and_then(|n| n.first()).copied()
                    .or_else(|| notes_arrays.first().and_then(|n| n.first()).copied())
                    .unwrap_or(bass_note);
                let wb_notes = walking_bass_notes(&[bass_note, chord[0], chord.get(1).copied().unwrap_or(bass_note)], next_root, seed, is_minor_chord(&[bass_note, chord[0]]));
                seed = seed.wrapping_add(1);
                for (bi, &bn) in wb_notes.iter().enumerate() {
                    let bt = chord_start + (bi as u32) * tpb / 4;
                    e(&mut evs, bt, &[0x92, bn, bv]);
                    // Note Off juste avant la prochaine note (ticks avant)
                    let off_tick = if bi < 3 { chord_start + ((bi + 1) as u32) * tpb / 4 - 1 } else { chord_end };
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

            // Drums
            if !drums_mute {
                let dv = sc(drums_vol, 127);
                let (kk, sn, hh, rm) = (sc(dv, 100), sc(dv, 90), sc(dv, 70), sc(dv, 60));
                let hh_up = sc(dv, 55);
                match cfg.pattern.as_str() {
                    "reggae" => {
                        if bar_beat == 2 { e(&mut evs, on_tick, &[0x99, KICK, kk]); e(&mut evs, on_tick, &[0x99, RIM, sc(dv, 90)]); }
                        e(&mut evs, on_tick, &[0x99, HH, hh]);
                    }
                    "jazz" => {
                        e(&mut evs, on_tick, &[0x99, 51, hh]); // ride
                        if bar_beat == 4 { e(&mut evs, on_tick, &[0x99, 44, sc(dv, 40)]); } // pedal HH
                        if bar_beat == 7 { e(&mut evs, on_tick, &[0x99, RIM, sc(dv, 50)]); }
                    }
                    "pop" => {
                        match bar_beat % 4 {
                            0 => { e(&mut evs, on_tick, &[0x99, KICK, sc(dv, 85)]); e(&mut evs, on_tick, &[0x99, HH, sc(dv, 50)]); }
                            1 => { e(&mut evs, on_tick, &[0x99, SNARE, sc(dv, 70)]); e(&mut evs, on_tick, &[0x99, HH, sc(dv, 50)]); }
                            2 => { e(&mut evs, on_tick, &[0x99, KICK, sc(dv, 75)]); }
                            3 => { e(&mut evs, on_tick, &[0x99, SNARE, sc(dv, 65)]); e(&mut evs, on_tick, &[0x99, HH, sc(dv, 50)]); }
                            _ => {}
                        }
                    }
                    "bossa" => {
                        match bar_beat % 4 {
                            0 => { e(&mut evs, on_tick, &[0x99, KICK, sc(dv, 55)]); }
                            1 => { e(&mut evs, on_tick, &[0x99, SNARE, sc(dv, 30)]); }
                            2 => { e(&mut evs, on_tick, &[0x99, KICK, sc(dv, 60)]); }
                            3 => { e(&mut evs, on_tick, &[0x99, KICK, sc(dv, 50)]); }
                            _ => {}
                        }
                        e(&mut evs, on_tick, &[0x99, HH, sc(dv, 45)]);
                    }
                    "onedrop" => {
                        match bar_beat % 4 {
                            0 => { e(&mut evs, on_tick, &[0x99, KICK, sc(dv, 90)]); }
                            2 => { e(&mut evs, on_tick, &[0x99, KICK, sc(dv, 90)]); e(&mut evs, on_tick, &[0x99, RIM, sc(dv, 65)]); }
                            _ => {}
                        }
                        e(&mut evs, on_tick, &[0x99, HH, sc(dv, 55)]);
                    }
                    _ => { // rock par défaut
                        match bar_beat % 4 {
                            0 => { e(&mut evs, on_tick, &[0x99, KICK, kk]); e(&mut evs, on_tick, &[0x99, HH, hh]); }
                            1 => { e(&mut evs, on_tick, &[0x99, SNARE, sn]); e(&mut evs, on_tick, &[0x99, HH, hh]); }
                            2 => { e(&mut evs, on_tick, &[0x99, KICK, kk]); }
                            3 => { e(&mut evs, on_tick, &[0x99, SNARE, sn]); e(&mut evs, on_tick, &[0x99, HH, hh]); e(&mut evs, on_tick, &[0x99, RIM, rm]); }
                            _ => {}
                        }
                    }
                }
                // Upbeat HH (pas pour bossa et jazz qui ont leur propre hi-hat)
                if !matches!(cfg.pattern.as_str(), "bossa" | "jazz") {
                    e(&mut evs, up_tick, &[0x99, HH, hh_up]);
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
