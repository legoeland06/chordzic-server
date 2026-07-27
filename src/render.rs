/// Render chord progression to WAV via MIDI file + fluidsynth batch.
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

fn write_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_be_bytes());
}

fn write_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

/// MIDI drum note numbers
const KICK: u8 = 36;
const SNARE: u8 = 38;
const HH_CLOSED: u8 = 42;
const RIMSHOT: u8 = 37;

/// Generate a Standard MIDI File with multi-track arrangement.
/// Lead (ch0), Bass (ch1), Nappes (ch2), Accent (ch4), Drums (ch9).
pub fn generate_smf(
    notes_arrays: &[Vec<u8>],
    beats: &[f64],
    tempo_bpm: u32,
    num_bars: usize,
) -> Vec<u8> {
    let tempo_us = (60_000_000u64 / tempo_bpm.max(1) as u64) as u32;
    let ppq = TICKS_PER_BEAT as u16;       // pulses per quarter note
    let _tpq = TICKS_PER_BEAT;               // ticks per quarter
    let half_tick = TICKS_PER_BEAT / 2;     // eighth note
    let qtr_tick = TICKS_PER_BEAT;

    let mut track = Vec::new();

    // ── Setup events (delta=0) ──
    // Tempo
    track.push(0x00);
    track.extend_from_slice(&[0xFF, 0x51, 0x03]);
    track.extend_from_slice(&tempo_us.to_be_bytes()[1..]);

    // Program changes: Lead, Bass, Nappes, Accent, Drums channel
    for &(ch, prog) in &[(0u8, 51u8), (1, 33), (2, 48), (4, 2), (9, 1)] {
        track.push(0x00);
        track.push(0xC0 | ch);
        track.push(prog);
    }

    // ── Chord loop ──
    for _bar in 0..num_bars {
        for (ci, notes) in notes_arrays.iter().enumerate() {
            let beat_count = if ci < beats.len() { beats[ci] } else { 4.0 };
            let total_ticks = (beat_count * TICKS_PER_BEAT as f64) as u32;
            let num_quarters = beat_count as u32;

            if notes.is_empty() {
                // Silence: just drums
                for b in 0..num_quarters {
                    let tick_pos = b * qtr_tick;
                    if tick_pos > 0 { write_vlq(&mut track, tick_pos); }
                    drum_hit(&mut track, b, num_quarters);
                }
                // Wait remaining
                let played = num_quarters * qtr_tick;
                if played < total_ticks {
                    write_vlq(&mut track, total_ticks - played);
                }
                continue;
            }

            // Separate bass note (first) from chord notes
            let bass_note = notes[0];
            let chord_notes: Vec<u8> = if notes.len() > 1 { notes[1..].to_vec() } else { vec![] };

            // ── Note On: Lead, Bass, Nappes, Accent ──
            // Lead (channel 0): chord notes
            for (i, &n) in chord_notes.iter().enumerate() {
                if i > 0 { track.push(0x00); }
                track.extend_from_slice(&[0x90, n, 80]);
            }
            // Bass (channel 1): root note
            track.push(0x00);
            track.extend_from_slice(&[0x91, bass_note, 90]);

            // Nappes (channel 2): chord notes, sustained
            for (i, &n) in chord_notes.iter().enumerate() {
                if i > 0 { track.push(0x00); }
                track.extend_from_slice(&[0x92, n, 60]);
            }

            // ── Sub-beat events: drums + accent ──
            for b in 0..num_quarters {
                let tick_pos = b * qtr_tick;
                if tick_pos > 0 { write_vlq(&mut track, tick_pos); }

                // Drums
                drum_hit(&mut track, b, num_quarters);

                // Accent (Bright Acoustic Piano, channel 4) on beats 2&4 (b=1,3)
                if b == 1 || b == 3 {
                    for (i, &n) in chord_notes.iter().enumerate() {
                        if i > 0 { track.push(0x00); }
                        track.extend_from_slice(&[0x94, n, 70]);
                    }
                }

                // Hi-hat on 8th notes (half-beat subdivisions)
                if half_tick > 0 {
                    write_vlq(&mut track, half_tick);
                    track.extend_from_slice(&[0x99, HH_CLOSED, 60]);
                }
            }

            // ── Wait remaining ticks after last drum hit ──
            // Drums + hi-hat already cover `num_quarters * qtr_tick + half_tick`
            let covered = num_quarters * qtr_tick + half_tick;
            if covered < total_ticks {
                write_vlq(&mut track, total_ticks - covered);
            } else if covered > total_ticks {
                // This shouldn't happen; clamp
                write_vlq(&mut track, total_ticks);
            }

            // ── Note Off: Lead, Bass, Nappes, Accent ──
            // Lead (channel 0)
            for (i, &n) in chord_notes.iter().enumerate() {
                if i > 0 { track.push(0x00); }
                track.extend_from_slice(&[0x80, n, 64]);
            }
            // Bass (channel 1)
            track.push(0x00);
            track.extend_from_slice(&[0x81, bass_note, 64]);
            // Nappes (channel 2)
            for (i, &n) in chord_notes.iter().enumerate() {
                if i > 0 { track.push(0x00); }
                track.extend_from_slice(&[0x82, n, 64]);
            }
            // Accent notes already ended naturally (short staccato)
        }
    }

    // End of Track
    track.push(0x00);
    track.extend_from_slice(&[0xFF, 0x2F, 0x00]);

    // ── Assemble SMF ──
    let mut smf = Vec::new();
    smf.extend_from_slice(b"MThd");
    write_u32(&mut smf, 6);
    write_u16(&mut smf, 0);           // format 0
    write_u16(&mut smf, 1);           // 1 track
    write_u16(&mut smf, ppq);
    smf.extend_from_slice(b"MTrk");
    write_u32(&mut smf, track.len() as u32);
    smf.extend_from_slice(&track);

    smf
}

/// Hit drums at a given beat position.
/// b: beat index (0-based), num_quarters: beats per chord
fn drum_hit(track: &mut Vec<u8>, b: u32, _num_quarters: u32) {
    match b % 4 {
        0 => {
            // Beat 1: Kick + Closed Hi-hat
            track.push(0x00);
            track.extend_from_slice(&[0x99, KICK, 100]);
            track.push(0x00);
            track.extend_from_slice(&[0x99, HH_CLOSED, 70]);
        }
        1 => {
            // Beat 2: Snare + Hi-hat
            track.push(0x00);
            track.extend_from_slice(&[0x99, SNARE, 90]);
            track.push(0x00);
            track.extend_from_slice(&[0x99, HH_CLOSED, 70]);
        }
        2 => {
            // Beat 3: Kick
            track.push(0x00);
            track.extend_from_slice(&[0x99, KICK, 80]);
        }
        3 => {
            // Beat 4: Snare + Hi-hat + Rimshot
            track.push(0x00);
            track.extend_from_slice(&[0x99, SNARE, 90]);
            track.push(0x00);
            track.extend_from_slice(&[0x99, HH_CLOSED, 70]);
            track.push(0x00);
            track.extend_from_slice(&[0x99, RIMSHOT, 60]);
        }
        _ => {}
    }
}

/// Render SMF to WAV using fluidsynth CLI.
pub fn render_wav(smf: &[u8], soundfont: &str) -> Result<Vec<u8>, String> {
    let mid_path = std::env::temp_dir().join("chordj_render.mid");
    let wav_path = std::env::temp_dir().join("chordj_render.wav");

    std::fs::write(&mid_path, smf).map_err(|e| format!("write mid: {e}"))?;

    let output = Command::new("fluidsynth")
        .arg("-F").arg(&wav_path)
        .arg("-T").arg("wav")
        .arg("-g").arg("1.0")
        .arg("-n").arg("-i")
        .arg(soundfont).arg(&mid_path)
        .output()
        .map_err(|e| format!("fluidsynth exec: {e}"))?;

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
