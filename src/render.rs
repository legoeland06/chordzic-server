/// Render chord progression — génération SMF Format 0 + render WAV.
///
/// Approche safe : on construit d'abord une liste d'événements (tick, bytes),
/// on la trie par tick, puis on sérialise en SMF avec un unique delta par
/// événement. Pas de delta 0 implicite, pas d'ambiguïté running status.
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

const KICK: u8 = 36; const SNARE: u8 = 38; const HH: u8 = 42; const RIM: u8 = 37;
const CH_LEAD: u8 = 0; const CH_BASS: u8 = 2; const CH_STR: u8 = 3; const CH_ACC: u8 = 4; const CH_DRUMS: u8 = 9;

// ─── SMF Format 0 ─────────────────────────────────────────────────────
// Chaque événement est construit avec son tick absolu. Au moment de la
// sérialisation, on calcule le delta = tick - prev_tick.
// Pas de delta 0 implicite, pas d'ambiguïté.
pub fn generate_smf_fmt0(
    notes_arrays: &[Vec<u8>],
    beats: &[f64],
    tempo_bpm: u32,
    _num_bars: usize,
) -> Vec<u8> {
    let tempo_us = (60_000_000u64 / tempo_bpm.max(1) as u64) as u32;
    let tpb = TICKS_PER_BEAT;
    let eighth = tpb / 2;

    // ── Construction de la liste d'événements ──────────────────────
    struct Ev { tick: u32, bytes: Vec<u8> }
    let mut evs: Vec<Ev> = Vec::new();

    fn e(evs: &mut Vec<Ev>, tick: u32, bytes: &[u8]) {
        evs.push(Ev { tick, bytes: bytes.to_vec() });
    }

    // Tempo
    e(&mut evs, 0, &[0xFF, 0x51, 0x03,
        ((tempo_us >> 16) & 0xFF) as u8,
        ((tempo_us >> 8)  & 0xFF) as u8,
        (tempo_us & 0xFF) as u8]);

    // Init PCs
    for &(ch, prog) in &[(CH_LEAD,51),(CH_BASS,33),(CH_STR,48),(CH_ACC,2),(CH_DRUMS,0)] {
        e(&mut evs, 0, &[0xC0|ch, prog]);
    }

    // Boucle accords
    let mut abs_tick = 0u32;
    for (ci, notes) in notes_arrays.iter().enumerate() {
        let bc = if ci < beats.len() { beats[ci] } else { 4.0 };
        let total_ticks = (bc * tpb as f64) as u32;
        let nq = bc as u32;

        if notes.is_empty() {
            abs_tick += total_ticks;
            continue;
        }

        let chord_start = abs_tick;
        let chord_end = abs_tick + total_ticks;
        abs_tick = chord_end;

        let bass = notes[0];
        let chord: &[u8] = if notes.len() > 1 { &notes[1..] } else { &[] };
        for &n in chord {
            e(&mut evs, chord_start, &[0x90, n, 80]);
        }
        e(&mut evs, chord_start, &[0x92, bass, 90]);
        for &n in chord {
            e(&mut evs, chord_start, &[0x93, n, 60]);
        }

        // Drums + Accent
        for b in 0..nq {
            let on_tick = chord_start + b * tpb;
            let up_tick = on_tick + eighth;

            // On-beat drums
            match b % 4 {
                0 => { e(&mut evs, on_tick, &[0x99, KICK, 100]); e(&mut evs, on_tick, &[0x99, HH, 70]); }
                1 => { e(&mut evs, on_tick, &[0x99, SNARE, 90]); e(&mut evs, on_tick, &[0x99, HH, 70]); }
                2 => { e(&mut evs, on_tick, &[0x99, KICK, 80]); }
                3 => { e(&mut evs, on_tick, &[0x99, SNARE, 90]); e(&mut evs, on_tick, &[0x99, HH, 70]); e(&mut evs, on_tick, &[0x99, RIM, 60]); }
                _ => {}
            }

            // Accent (temps 2&4)
            if b == 1 || b == 3 {
                for &n in chord {
                    e(&mut evs, on_tick, &[0x94, n, 70]);
                    e(&mut evs, on_tick + 1, &[0x84, n, 64]);
                }
            }

            // Upbeat HH
            e(&mut evs, up_tick, &[0x99, HH, 60]);
        }

        // Note Offs
        for &n in chord {
            e(&mut evs, chord_end, &[0x80, n, 64]);
        }
        e(&mut evs, chord_end, &[0x82, bass, 64]);
        for &n in chord {
            e(&mut evs, chord_end, &[0x83, n, 64]);
        }
    }

    // ── Sérialisation ──────────────────────────────────────────────
    // Trier par tick
    evs.sort_by_key(|e| e.tick);

    let mut t = Vec::new();
    let mut prev_tick = 0u32;
    for ev in &evs {
        let delta = ev.tick - prev_tick;
        write_vlq(&mut t, delta);
        t.extend_from_slice(&ev.bytes);
        prev_tick = ev.tick;
    }

    // EOT
    write_vlq(&mut t, 0);
    t.extend_from_slice(&[0xFF, 0x2F, 0x00]);

    // ── Assemble SMF ───────────────────────────────────────────────
    let mut smf = Vec::new();
    smf.extend_from_slice(b"MThd");
    write_u32(&mut smf, 6); write_u16(&mut smf, 0); write_u16(&mut smf, 1); write_u16(&mut smf, tpb as u16);
    smf.extend_from_slice(b"MTrk"); write_u32(&mut smf, t.len() as u32); smf.extend_from_slice(&t);
    smf
}

// ─── Render WAV via fluidsynth CLI ──────────────────────────────────────
/// Coupe la fin du WAV au dernier échantillon non-silencieux.
fn trim_wav_tail(wav: &[u8]) -> Vec<u8> {
    use hound::WavReader;
    let Ok(mut reader) = WavReader::new(std::io::Cursor::new(wav)) else {
        return wav.to_vec(); // fallback: retourner tel quel
    };
    let spec = reader.spec();
    let samples: Vec<i16> = reader.samples::<i16>().filter_map(|s| s.ok()).collect();
    if samples.is_empty() { return wav.to_vec(); }

    // Dernier échantillon dont l'amplitude dépasse le seuil
    let threshold: i16 = 300;
    let last = samples.iter().rposition(|&s| s.abs() > threshold)
        .unwrap_or(samples.len().saturating_sub(1));

    // 50ms de queue naturelle après la dernière attaque
    let extra = (spec.sample_rate as usize * 50 / 1000) * spec.channels as usize;
    let end = (last + extra).min(samples.len());

    let mut out = Vec::new();
    if let Ok(mut w) = hound::WavWriter::new(std::io::Cursor::new(&mut out), spec) {
        for &s in &samples[..end] {
            let _ = w.write_sample(s);
        }
        let _ = w.finalize();
    }
    if out.is_empty() { wav.to_vec() } else { out }
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

    // Couper la queue silencieuse (réverb naturelle de la SoundFont)
    Ok(trim_wav_tail(&wav))
}
