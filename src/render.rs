/// Render chord progression to WAV via MIDI file + fluidsynth batch.
///
/// SMF Format 1 (multi-track) — chaque instrument a son propre track.
/// Évite les bugs de running status inter-canaux de FluidSynth.
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

// ─── Per-track builder (suit sa propre position absolue en ticks) ──────
struct Track {
    data: Vec<u8>,
    abs: u32,
    started: bool,
}

impl Track {
    fn new() -> Self {
        Self { data: Vec::new(), abs: 0, started: false }
    }

    /// Avance à `tick` (absolu) en émettant un delta VLQ.
    /// TOUJOURS émettre un delta (même 0), y compris entre deux événements
    /// au même tick — sinon un status byte MSB=1 (comme 0x90) est mangé
    /// comme byte VLQ par le parser MIDI.
    fn advance(&mut self, tick: u32) {
        if !self.started {
            write_vlq(&mut self.data, tick); // tick=0 → 0x00
            self.abs = tick;
            self.started = true;
        } else if tick >= self.abs {
            let delta = tick - self.abs;
            if delta > 0 {
                write_vlq(&mut self.data, delta);
            } else {
                self.data.push(0x00); // delta 0 explicite
            }
            self.abs = tick;
        }
    }

    /// Émet des bytes MIDI bruts à `tick` absolu.
    fn emit(&mut self, tick: u32, bytes: &[u8]) {
        self.advance(tick);
        self.data.extend_from_slice(bytes);
    }

    fn pc(&mut self, tick: u32, ch: u8, prog: u8)  { self.emit(tick, &[0xC0 | ch, prog]); }
    fn on(&mut self, tick: u32, ch: u8, note: u8, vel: u8)  { self.emit(tick, &[0x90 | ch, note, vel]); }
    fn off(&mut self, tick: u32, ch: u8, note: u8, vel: u8) { self.emit(tick, &[0x80 | ch, note, vel]); }

    /// End of Track — avance d'abord à `end_tick` pour les silences finaux.
    /// NOTE: `advance()` émet déjà le delta (même 0). Pas de `0x00` en plus.
    fn eot(&mut self, end_tick: u32) {
        self.advance(end_tick);
        self.data.extend_from_slice(&[0xFF, 0x2F, 0x00]);
    }
}

// ─── GM Drum Notes ─────────────────────────────────────────────────────
const KICK: u8 = 36;
const SNARE: u8 = 38;
const HH:   u8 = 42;
const RIM:  u8 = 37;

// ─── Generate SMF Format 1 ────────────────────────────────────────────
//
// 6 tracks: Conductor | Lead(ch0) | Bass(ch2) | Strings(ch3) | Accent(ch4) | Drums(ch9)
//
// notes_arrays: chaque élément = [bass_midi, chord_note1, chord_note2, ...]
// beats: durée de chaque accord en noires
// tempo_bpm: tempo en BPM
//
pub fn generate_smf(
    notes_arrays: &[Vec<u8>],
    beats: &[f64],
    tempo_bpm: u32,
    _num_bars: usize,
) -> Vec<u8> {
    let tempo_us = (60_000_000u64 / tempo_bpm.max(1) as u64) as u32;
    let tpb = TICKS_PER_BEAT;        // 480 ticks/noire
    let eighth = tpb / 2;            // 240 ticks/croche

    // ── Calcul des bornes temporelles de chaque accord ──────────────
    let mut chord_start = Vec::new();
    let mut chord_end = Vec::new();
    let mut abs: u32 = 0;
    for (i, _) in notes_arrays.iter().enumerate() {
        let bc = if i < beats.len() { beats[i] } else { 4.0 };
        let total = (bc * tpb as f64) as u32;
        chord_start.push(abs);
        abs += total;
        chord_end.push(abs);
    }
    let total_len = abs;

    // 6 tracks : 0=conductor, 1=lead, 2=bass, 3=strings, 4=accent, 5=drums
    let mut t: Vec<Track> = (0..6).map(|_| Track::new()).collect();

    // ═══════════════════════════════════════════════════════════════
    //  TRACK 0 — Conducteur (tempo + fin)
    // ═══════════════════════════════════════════════════════════════
    t[0].emit(0, &[
        0xFF, 0x51, 0x03,
        ((tempo_us >> 16) & 0xFF) as u8,
        ((tempo_us >> 8)  & 0xFF) as u8,
        (tempo_us & 0xFF) as u8,
    ]);
    t[0].eot(total_len);

    // ═══════════════════════════════════════════════════════════════
    //  TRACK 1 — Lead (ch0, Synth Brass 51)
    //           Joue les notes d'accord (notes[1..]) sur toute la durée
    // ═══════════════════════════════════════════════════════════════
    t[1].pc(0, 0, 51);
    for (ci, notes) in notes_arrays.iter().enumerate() {
        if notes.len() < 2 { continue; }
        for &n in &notes[1..] { t[1].on(chord_start[ci], 0, n, 80); }
        for &n in &notes[1..] { t[1].off(chord_end[ci], 0, n, 64); }
    }
    t[1].eot(total_len);

    // ═══════════════════════════════════════════════════════════════
    //  TRACK 2 — Bass (ch2, Electric Bass finger 33)
    //           Joue la fondamentale (notes[0]) sur toute la durée
    // ═══════════════════════════════════════════════════════════════
    t[2].pc(0, 2, 33);
    for (ci, notes) in notes_arrays.iter().enumerate() {
        if notes.is_empty() { continue; }
        t[2].on(chord_start[ci], 2, notes[0], 90);
        t[2].off(chord_end[ci], 2, notes[0], 64);
    }
    t[2].eot(total_len);

    // ═══════════════════════════════════════════════════════════════
    //  TRACK 3 — Strings (ch3, String Ensemble 48)
    //           Joue les notes d'accord (notes[1..]) sur toute la durée
    // ═══════════════════════════════════════════════════════════════
    t[3].pc(0, 3, 48);
    for (ci, notes) in notes_arrays.iter().enumerate() {
        if notes.len() < 2 { continue; }
        for &n in &notes[1..] { t[3].on(chord_start[ci], 3, n, 60); }
        for &n in &notes[1..] { t[3].off(chord_end[ci], 3, n, 64); }
    }
    t[3].eot(total_len);

    // ═══════════════════════════════════════════════════════════════
    //  TRACK 4 — Accent (ch4, Bright Acoustic Piano 2)
    //           Staccato sur temps 2 et 4 (index 1 et 3)
    //           Tous les Note On puis tous les Note Off (pas d'entrelacement)
    // ═══════════════════════════════════════════════════════════════
    t[4].pc(0, 4, 2);
    for (ci, notes) in notes_arrays.iter().enumerate() {
        if notes.len() < 2 { continue; }
        let start = chord_start[ci];
        let nq = if ci < beats.len() { beats[ci] as u32 } else { 4 };
        for b in 0..nq {
            if b == 1 || b == 3 {
                let hit = start + b * tpb;
                for &n in &notes[1..] { t[4].on(hit, 4, n, 70); }
                for &n in &notes[1..] { t[4].off(hit + 1, 4, n, 64); }
            }
        }
    }
    t[4].eot(total_len);

    // ═══════════════════════════════════════════════════════════════
    //  TRACK 5 — Drums (ch9, GM Standard Kit)
    //           Pattern rock : kick/snare/hh/rim sur les temps
    //           + hi-hat contretemps croche
    //           (silence si notes vides, comme l'original)
    // ═══════════════════════════════════════════════════════════════
    t[5].pc(0, 9, 1);
    for (ci, notes) in notes_arrays.iter().enumerate() {
        if notes.is_empty() { continue; }
        let start = chord_start[ci];
        let nq = if ci < beats.len() { beats[ci] as u32 } else { 4 };
        for b in 0..nq {
            let on_tick = start + b * tpb;
            let up_tick = on_tick + eighth;
            match b % 4 {
                0 => { t[5].on(on_tick, 9, KICK, 100); t[5].on(on_tick, 9, HH, 70); }
                1 => { t[5].on(on_tick, 9, SNARE, 90); t[5].on(on_tick, 9, HH, 70); }
                2 => { t[5].on(on_tick, 9, KICK, 80); }
                3 => { t[5].on(on_tick, 9, SNARE, 90); t[5].on(on_tick, 9, HH, 70); t[5].on(on_tick, 9, RIM, 60); }
                _ => {}
            }
            t[5].on(up_tick, 9, HH, 60);
        }
    }
    t[5].eot(total_len);

    // ═══════════════════════════════════════════════════════════════
    //  ASSEMBLAGE SMF
    // ═══════════════════════════════════════════════════════════════
    let mut smf = Vec::new();
    smf.extend_from_slice(b"MThd");
    write_u32(&mut smf, 6);
    write_u16(&mut smf, 1);        // Format 1 (multi-track)
    write_u16(&mut smf, 6);        // 6 tracks
    write_u16(&mut smf, tpb as u16);

    for track in &t {
        smf.extend_from_slice(b"MTrk");
        write_u32(&mut smf, track.data.len() as u32);
        smf.extend_from_slice(&track.data);
    }

    smf
}

// ─── Render WAV via fluidsynth CLI ──────────────────────────────────────
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
