/// Render chord progression to WAV via MIDI file + fluidsynth batch.
use std::process::Command;

const TICKS_PER_BEAT: u32 = 480;

/// VLQ encode a value into the buffer (variable-length quantity for delta times).
fn write_vlq(buf: &mut Vec<u8>, mut v: u32) {
    // Build bytes in reverse
    let mut bytes = Vec::new();
    bytes.push((v & 0x7F) as u8);
    v >>= 7;
    while v > 0 {
        bytes.push(((v & 0x7F) | 0x80) as u8);
        v >>= 7;
    }
    buf.extend(bytes.into_iter().rev());
}

/// Write a 16-bit big-endian value.
fn write_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_be_bytes());
}

/// Write a 32-bit big-endian value.
fn write_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

/// Generate a Standard MIDI File from chord events.
/// Returns raw SMF bytes.
pub fn generate_smf(
    notes_arrays: &[Vec<u8>],  // per-chord MIDI note numbers
    beats: &[f64],              // per-chord duration in beats
    tempo_bpm: u32,             // BPM
    num_bars: usize,            // how many bars/loops
) -> Vec<u8> {
    let tempo_us = (60_000_000u64 / tempo_bpm.max(1) as u64) as u32; // µs/qn
    let ticks_per_qn = TICKS_PER_BEAT as u16;

    // ── Build track events ──
    let mut track = Vec::new();

    // Tempo meta event (delta=0)
    track.push(0x00);
    track.extend_from_slice(&[0xFF, 0x51, 0x03]);
    track.extend_from_slice(&tempo_us.to_be_bytes()[1..]); // 3 bytes

    // Program Change: channel 0 (Lead) -> program 51 (Synth Strings 1)
    track.push(0x00);
    track.push(0xC0);
    track.push(51);

    // Program Change: channel 1 (Bass) -> program 33 (Electric Bass)
    track.push(0x00);
    track.push(0xC1);
    track.push(33);

    // Program Change: channel 2 (Nappes) -> program 48 (String Ensemble)
    track.push(0x00);
    track.push(0xC2);
    track.push(48);

    // Program Change: channel 9 (Drums) -> no program (drums are on ch10)
    track.push(0x00);
    track.push(0xC9);
    track.push(1);

    // Loop through chords for the specified number of bars
    for _bar in 0..num_bars {
        for (ci, notes) in notes_arrays.iter().enumerate() {
            let beat_count = if ci < beats.len() { beats[ci] } else { 4.0 };
            let duration_ticks = (beat_count * TICKS_PER_BEAT as f64) as u32;

            if notes.is_empty() {
                // Silence: skip (drums could be added here)
                continue;
            }

            // Note On for all notes in this chord
            for &note in notes {
                // Lead (channel 0)
                track.push(0x00);
                track.extend_from_slice(&[0x90, note, 80]);
            }

            // Wait for chord duration
            write_vlq(&mut track, duration_ticks);

            // Note Off for all notes
            for &note in notes {
                track.extend_from_slice(&[0x80, note, 64]);
                // Delta time 0 for subsequent offs
                track.push(0x00);
            }
        }
    }

    // End of Track
    track.push(0x00);
    track.extend_from_slice(&[0xFF, 0x2F, 0x00]);

    // ── Assemble SMF ──
    let mut smf = Vec::new();

    // Header chunk
    smf.extend_from_slice(b"MThd");
    write_u32(&mut smf, 6);           // header length
    write_u16(&mut smf, 1);           // format 1
    write_u16(&mut smf, 1);           // 1 track
    write_u16(&mut smf, ticks_per_qn); // ticks/quarter

    // Track chunk
    smf.extend_from_slice(b"MTrk");
    write_u32(&mut smf, track.len() as u32); // track length
    smf.extend_from_slice(&track);

    smf
}

/// Render an SMF to WAV using fluidsynth CLI.
/// Returns the WAV bytes.
pub fn render_wav(smf: &[u8], soundfont: &str) -> Result<Vec<u8>, String> {
    // Write SMF to temp file
    let mid_path = std::env::temp_dir().join("chordj_render.mid");
    let wav_path = std::env::temp_dir().join("chordj_render.wav");

    std::fs::write(&mid_path, smf).map_err(|e| format!("write mid: {e}"))?;

    // Run fluidsynth
    let status = Command::new("fluidsynth")
        .arg("-F")
        .arg(&wav_path)
        .arg("-T")
        .arg("wav")
        .arg("-g")
        .arg("1.0")
        .arg(soundfont)
        .arg(&mid_path)
        .status()
        .map_err(|e| format!("fluidsynth exec: {e}"))?;

    if !status.success() {
        let _ = std::fs::remove_file(&mid_path);
        return Err("fluidsynth failed".into());
    }

    // Read WAV
    let wav = std::fs::read(&wav_path).map_err(|e| format!("read wav: {e}"))?;

    // Cleanup
    let _ = std::fs::remove_file(&mid_path);
    let _ = std::fs::remove_file(&wav_path);

    Ok(wav)
}
