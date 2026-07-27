/// Render chord progression to WAV via MIDI file + fluidsynth batch.
/// Format 0 (single track) avec status byte explicite pour chaque evenement.
use std::process::Command;

const TICKS_PER_BEAT: u32 = 480;

fn write_vlq(buf: &mut Vec<u8>, mut v: u32) {
    let mut bytes = Vec::new();
    bytes.push((v & 0x7F) as u8);
    v >>= 7;
    while v > 0 {
        bytes.push(((v & 0x7F) | 0x80) as u8);
        v >>= 7;
    }
    buf.extend(bytes.into_iter().rev());
}

fn write_u16(buf: &mut Vec<u8>, v: u16) { buf.extend_from_slice(&v.to_be_bytes()); }
fn write_u32(buf: &mut Vec<u8>, v: u32) { buf.extend_from_slice(&v.to_be_bytes()); }

const KICK: u8 = 36; const SNARE: u8 = 38; const HH: u8 = 42; const RIM: u8 = 37;

/// Generate SMF format 0 — un seul track avec tous les canaux.
pub fn generate_smf(
    notes_arrays: &[Vec<u8>],
    beats: &[f64],
    tempo_bpm: u32,
    _num_bars: usize,
) -> Vec<u8> {
    let tempo_us = (60_000_000u64 / tempo_bpm.max(1) as u64) as u32;
    let mut t = Vec::new();

    // ── Setup ──
    t.extend_from_slice(&[0x00, 0xFF, 0x51, 0x03]);
    t.extend_from_slice(&tempo_us.to_be_bytes()[1..]);
    for &(ch, prog) in &[(0u8, 51u8), (1, 33), (2, 48), (4, 2), (9, 1)] {
        t.extend_from_slice(&[0x00, 0xC0 | ch, prog]);
    }

    // ── Chord loop ──
    for (ci, notes) in notes_arrays.iter().enumerate() {
        let beat_count = if ci < beats.len() { beats[ci] } else { 4.0 };
        let total_ticks = (beat_count * TICKS_PER_BEAT as f64) as u32;
        let nq = beat_count as u32;

        if notes.is_empty() { write_vlq(&mut t, total_ticks); continue; }

        let bass = notes[0];
        let chord: Vec<u8> = if notes.len() > 1 { notes[1..].to_vec() } else { vec![] };

        // ── Note Ons (tous avec status byte explicite, delta 0 ou 1) ──
        for &n in &chord { t.extend_from_slice(&[0x00, 0x90, n, 80]); }
        t.extend_from_slice(&[0x01, 0x91, bass, 90]);
        for &n in &chord { t.extend_from_slice(&[0x01, 0x92, n, 60]); }

        // ── Sub-beat events: drums + accent + hi-hat ──
        // REGLE : apres un VLQ, le prochain byte DOIT etre un status byte (MSB=1).
        // Jamais de delta-0 (\x00) entre un VLQ et un status.
        let mut pos = 0u32;
        for b in 0..nq {
            let target = b * 480;
            if target > pos { write_vlq(&mut t, target - pos); pos = target; }

            // Drums on the beat (ch9) — chaque note a son delta 0
            match b % 4 {
                0 => { t.extend_from_slice(&[0x99, KICK, 100, 0x00, HH, 70]); }
                1 => { t.extend_from_slice(&[0x99, SNARE, 90, 0x00, HH, 70]); }
                2 => { t.extend_from_slice(&[0x99, KICK, 80]); }
                3 => { t.extend_from_slice(&[0x99, SNARE, 90, 0x00, HH, 70, 0x00, RIM, 60]); }
                _ => {}
            }

            // Accent on beats 2&4 (ch4) — nouveau status byte 0x94
            if b == 1 || b == 3 {
                // 0x00 serait une data byte pour le running status ch9
                // On utilise 0x01 pour avancer d'1 tick + nouveau status
                for &n in &chord { t.extend_from_slice(&[0x01, 0x94, n, 70]); }
            }

            // Hi-hat off-beat — VLQ + status 0x99 direct
            write_vlq(&mut t, 240);
            t.extend_from_slice(&[0x99, HH, 60]);
            pos = target + 240;
        }

        // ── Advance to end of chord & Note Offs ──
        if total_ticks > pos { write_vlq(&mut t, total_ticks - pos); }

        // Note Offs avec status byte explicite
        for &n in &chord { t.extend_from_slice(&[0x00, 0x80, n, 64]); }
        t.extend_from_slice(&[0x01, 0x81, bass, 64]);
        for &n in &chord { t.extend_from_slice(&[0x01, 0x82, n, 64]); }
    }

    t.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);

    let mut smf = Vec::new();
    smf.extend_from_slice(b"MThd");
    write_u32(&mut smf, 6);
    write_u16(&mut smf, 0);           // format 0
    write_u16(&mut smf, 1);           // 1 track
    write_u16(&mut smf, TICKS_PER_BEAT as u16);
    smf.extend_from_slice(b"MTrk");
    write_u32(&mut smf, t.len() as u32);
    smf.extend_from_slice(&t);
    smf
}

pub fn render_wav(smf: &[u8], soundfont: &str) -> Result<Vec<u8>, String> {
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
    Ok(wav)
}
