/// Render chord progression to WAV via MIDI file + fluidsynth batch.
/// Chaque instrument a son propre track (evite les conflits de running status).
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

/// MIDI drum notes
const KICK: u8 = 36;
const SNARE: u8 = 38;
const HH: u8 = 42;
const RIM: u8 = 37;

/// Generate SMF with one track per channel (format 1).
pub fn generate_smf(
    notes_arrays: &[Vec<u8>],
    beats: &[f64],
    tempo_bpm: u32,
    _num_bars: usize,
) -> Vec<u8> {
    let tempo_us = (60_000_000u64 / tempo_bpm.max(1) as u64) as u32;
    let ppq = TICKS_PER_BEAT as u16;
    let half_tick = TICKS_PER_BEAT / 2;
    let qtr_tick = TICKS_PER_BEAT;

    // Build individual tracks
    let trk_tempo = make_tempo_track(tempo_us);             // Track 0
    let trk_lead = make_note_track(0, 51, notes_arrays, beats, qtr_tick, half_tick, true);  // Track 1
    let trk_bass = make_note_track(1, 33, notes_arrays, beats, qtr_tick, half_tick, false); // Track 2
    let trk_nappe = make_note_track(2, 48, notes_arrays, beats, qtr_tick, half_tick, true); // Track 3
    let trk_drums = make_drum_track(notes_arrays, beats, qtr_tick, half_tick);              // Track 4
    let trk_accent = make_accent_track(4, 2, notes_arrays, beats, qtr_tick, half_tick);     // Track 5

    let tracks = [trk_tempo, trk_lead, trk_bass, trk_nappe, trk_drums, trk_accent];
    let n = tracks.len() as u16;

    let mut smf = Vec::new();
    smf.extend_from_slice(b"MThd");
    write_u32(&mut smf, 6);
    write_u16(&mut smf, 1);  // format 1 (multi-track)
    write_u16(&mut smf, n);  // number of tracks
    write_u16(&mut smf, ppq);
    for track in &tracks {
        smf.extend_from_slice(b"MTrk");
        write_u32(&mut smf, track.len() as u32);
        smf.extend_from_slice(track);
    }
    smf
}

fn make_tempo_track(tempo_us: u32) -> Vec<u8> {
    let mut t = Vec::new();
    t.push(0x00);
    t.extend_from_slice(&[0xFF, 0x51, 0x03]);
    t.extend_from_slice(&tempo_us.to_be_bytes()[1..]);
    // Time signature 4/4
    t.extend_from_slice(&[0x00, 0xFF, 0x58, 0x04, 0x04, 0x02, 0x18, 0x08]);
    t.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    t
}

/// Create a note track (Lead, Bass, Nappes)
fn make_note_track(
    channel: u8, program: u8,
    notes_arrays: &[Vec<u8>],
    beats: &[f64],
    qtr_tick: u32, _half_tick: u32,
    use_chord: bool,  // true = joue notes[1..], false = joue notes[0]
) -> Vec<u8> {
    let mut t = Vec::new();

    // Program change
    t.push(0x00);
    t.push(0xC0 | channel);
    t.push(program);

    for (ci, notes) in notes_arrays.iter().enumerate() {
        let beat_count = if ci < beats.len() { beats[ci] } else { 4.0 };
        let total_ticks = (beat_count * TICKS_PER_BEAT as f64) as u32;
        let notes_to_play: Vec<u8> = if use_chord && notes.len() > 1 {
            notes[1..].to_vec()
        } else if !notes.is_empty() {
            vec![notes[0]]
        } else {
            vec![]
        };

        if notes_to_play.is_empty() {
            // Silence: just advance time
            write_vlq(&mut t, total_ticks);
            continue;
        }

        // Note Ons
        for (i, &n) in notes_to_play.iter().enumerate() {
            if i > 0 { t.push(0x00); }  // delta 0 for simultaneous notes (safe: same channel)
            t.extend_from_slice(&[0x90 | channel, n, 80]);
        }

        // Wait
        write_vlq(&mut t, total_ticks);

        // Note Offs (running status, no redundant status bytes)
        for (i, &n) in notes_to_play.iter().enumerate() {
            if i == 0 {
                t.extend_from_slice(&[0x80 | channel, n, 64]);
            } else {
                t.extend_from_slice(&[0x00, n, 64]);  // delta 0, data bytes only
            }
        }
    }

    t.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    t
}

/// Drums track (channel 9)
fn make_drum_track(
    notes_arrays: &[Vec<u8>],
    beats: &[f64],
    qtr_tick: u32, half_tick: u32,
) -> Vec<u8> {
    let mut t = Vec::new();
    // Program change (optional for drums, but set anyway)
    t.push(0x00);
    t.push(0xC9);
    t.push(1);

    for (ci, _notes) in notes_arrays.iter().enumerate() {
        let beat_count = if ci < beats.len() { beats[ci] } else { 4.0 };
        let total_ticks = (beat_count * TICKS_PER_BEAT as f64) as u32;
        let num_q = beat_count as u32;

        let mut last_pos = 0u32;
        for b in 0..num_q {
            let tick_pos = b * qtr_tick;
            let delta = if b == 0 { 0 } else { tick_pos - last_pos };
            if delta > 0 { write_vlq(&mut t, delta); }

            // Drums on the beat (running status on ch9)
            match b % 4 {
                0 => { t.extend_from_slice(&[0x99, KICK, 100]); t.push(0x00); t.extend_from_slice(&[HH, 70]); }
                1 => { t.extend_from_slice(&[0x99, SNARE, 90]); t.push(0x00); t.extend_from_slice(&[HH, 70]); }
                2 => { t.extend_from_slice(&[0x99, KICK, 80]); }
                3 => { t.extend_from_slice(&[0x99, SNARE, 90]); t.push(0x00); t.extend_from_slice(&[HH, 70]); t.push(0x00); t.extend_from_slice(&[RIM, 60]); }
                _ => {}
            }

            // Hi-hat on 8th note (off-beat)
            write_vlq(&mut t, half_tick);
            t.extend_from_slice(&[0x99, HH, 60]);  // Note On with running status

            last_pos = tick_pos + half_tick;
        }

        // Advance to end of chord
        if total_ticks > last_pos {
            write_vlq(&mut t, total_ticks - last_pos);
        }
    }

    t.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    t
}

/// Accent track (Bright Acoustic Piano on 2&4)
fn make_accent_track(
    channel: u8, program: u8,
    notes_arrays: &[Vec<u8>],
    beats: &[f64],
    qtr_tick: u32, _half_tick: u32,
) -> Vec<u8> {
    let mut t = Vec::new();
    t.push(0x00);
    t.push(0xC0 | channel);
    t.push(program);

    for (ci, notes) in notes_arrays.iter().enumerate() {
        let beat_count = if ci < beats.len() { beats[ci] } else { 4.0 };
        let total_ticks = (beat_count * TICKS_PER_BEAT as f64) as u32;
        let chord_notes: Vec<u8> = if notes.len() > 1 { notes[1..].to_vec() } else { vec![] };
        let num_q = beat_count as u32;

        // Accent on beats 2 and 4 (index 1, 3)
        for b in 0..num_q {
            let tick_pos = b * qtr_tick;
            if tick_pos > 0 { write_vlq(&mut t, tick_pos); }

            if b == 1 || b == 3 {
                // Accent notes (staccato: short note-off after)
                for (i, &n) in chord_notes.iter().enumerate() {
                    if i > 0 { t.push(0x00); }
                    t.extend_from_slice(&[0x90 | channel, n, 70]);
                }
                // Very short duration (1 tick)
                write_vlq(&mut t, 1);
                for (i, &n) in chord_notes.iter().enumerate() {
                    if i == 0 { t.extend_from_slice(&[0x80 | channel, n, 64]); }
                    else { t.extend_from_slice(&[0x00, n, 64]); }
                }
            } else {
                write_vlq(&mut t, qtr_tick);
            }
        }

        // Fill remaining time to chord boundary
        let covered = num_q * qtr_tick;
        if covered < total_ticks {
            write_vlq(&mut t, total_ticks - covered);
        }
    }

    t.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    t
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
