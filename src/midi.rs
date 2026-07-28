/// MIDI temps réel — LiveTrack, Live, play_seq, drum_hit, helpers MIDI.
use midir::{MidiOutput, MidiOutputConnection};
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU16, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use crate::patterns::{sc, DRUM_HH, DRUM_KICK, DRUM_RIDE, DRUM_RIM, DRUM_SNARE, HH_8TH, HH_BEAT, PAT_BOSSA, PAT_JAZZ, PAT_ONEDROP, PAT_POP, PAT_REGGAE};
use crate::walking::{generate_walking_bass, is_minor, MIN_NOTE};

pub type MidiHandle = Arc<Mutex<MidiOutputConnection>>;

// ─── Tracks ──────────────────────────────────────────────────────────────
pub const TRACK_LEAD: usize = 0;
pub const TRACK_BASS: usize = 1;
pub const TRACK_STR: usize = 2;
pub const TRACK_DRUMS: usize = 3;
pub const TRACK_ACCENT: usize = 4;

pub struct LiveTrack {
    pub channel: u8,
    pub program: AtomicU16,
    pub volume: AtomicU8,
    pub mute: AtomicBool,
}

impl LiveTrack {
    pub fn new(ch: u8, pg: u16, vol: u8) -> Self {
        Self { channel: ch, program: AtomicU16::new(pg), volume: AtomicU8::new(vol), mute: AtomicBool::new(false) }
    }
}

pub struct Live {
    pub tracks: [LiveTrack; 5],
    pub pattern: AtomicU8,
    pub tempo: AtomicU16,
    pub stop: AtomicBool,
    pub sig: AtomicU16,
    pub walking: AtomicBool,
    pub master_vol: AtomicU8,
    pub use432: AtomicBool,
    pub loop_offset: AtomicI32,
    pub use_loops: AtomicBool,
    pub loop_name: Mutex<String>,
    pub loop_volume: AtomicU8,
}

// ─── Note MIDI ──────────────────────────────────────────────────────────
pub fn note_midi(s: &str) -> Result<u8, String> {
    let s = s.trim();
    let (nl, np) = if s.len() > 1 && (s.as_bytes()[1] == b'#' || s.as_bytes()[1] == b'b') {
        (2, &s[..2])
    } else {
        (1, &s[..1])
    };
    let o: i32 = s[nl..].parse().map_err(|_| "o")?;
    let u = np.to_uppercase();
    let n = match u.as_str() {
        "DB" => "C#", "EB" => "D#", "GB" => "F#", "AB" => "G#", "BB" => "A#", _ => &u,
    };
    let i = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
        .iter()
        .position(|x| x == &n)
        .ok_or("?")?;
    let m = (o + 1) * 12 + i as i32;
    if m < 0 || m > 127 { return Err("o".into()) }
    Ok(m as u8)
}

pub fn notes_from_vec(notes: &[String]) -> Vec<u8> {
    let mut v = vec![];
    for n in notes {
        if let Ok(x) = note_midi(n) { v.push(x) }
    }
    v
}

// ─── Init MIDI ──────────────────────────────────────────────────────────
pub fn init_midi() -> Option<MidiHandle> {
    let mo = MidiOutput::new("cs").ok()?;
    let p = mo.ports();
    if p.is_empty() { eprintln!("no port"); return None }
    println!("Ports:");
    for (i, x) in p.iter().enumerate() {
        if let Ok(n) = mo.port_name(x) { println!(" [{i}] {n}") }
    }
    let i: usize = if let Ok(e) = std::env::var("MIDI_PORT") { e.parse().unwrap_or(2) } else { 2 };
    if i >= p.len() { eprintln!("port {i} invalid"); return None }
    println!("Connecte {}", mo.port_name(&p[i]).unwrap_or_default());
    mo.connect(&p[i], "cs").ok().map(|c| Arc::new(Mutex::new(c)))
}

// ─── Helpers MIDI ──────────────────────────────────────────────────────
pub fn snd(c: &mut MidiOutputConnection, m: &[u8]) {
    if let Err(e) = c.send(m) { eprintln!("⚠️{e}") }
}
pub fn cc(c: &mut MidiOutputConnection, ch: u8, ctl: u8, v: u8) { snd(c, &[0xB0 | ch, ctl, v]) }
pub fn pc(c: &mut MidiOutputConnection, ch: u8, v: u8) { snd(c, &[0xC0 | ch, v]) }
pub fn no(c: &mut MidiOutputConnection, ch: u8, n: u8, v: u8) { snd(c, &[0x90 | ch, n, v]) }
pub fn no_mv(c: &mut MidiOutputConnection, ch: u8, n: u8, v: u8, mv: u8) {
    snd(c, &[0x90 | ch, n, ((v as u16 * mv as u16) / 127).min(127) as u8])
}
pub fn rch(c: &mut MidiOutputConnection) {
    for &ch in &[0u8, 2, 3, 4, 9] { cc(c, ch, 123, 0) }
}
pub fn pb(c: &mut MidiOutputConnection, ch: u8, val: u16) {
    let lsb = (val & 127) as u8;
    let msb = ((val >> 7) & 127) as u8;
    snd(c, &[0xE0 | ch, lsb, msb])
}

// ─── Drum hit ──────────────────────────────────────────────────────────
fn drum_hit(c: &mut MidiOutputConnection, beat: u64, pat: u8, on_beat: bool, on_eighth: bool, bars: u64, vol: u8, mv: u8) {
    if !on_beat && !on_eighth { return }
    let b = beat % bars;
    let v = sc(vol, mv);
    let hh = sc(v, HH_BEAT); let h8 = sc(v, HH_8TH); let h55 = sc(v, 55);
    let h45 = sc(v, 45); let h40 = sc(v, 10); let h60 = sc(v, 60); let h65 = sc(v, 65);
    match pat {
        PAT_REGGAE => if on_beat {
            match b {
                0 | 1 | 3 => { no(c, 9, DRUM_HH, h60); }
                2 => { no(c, 9, DRUM_KICK, sc(v, 120)); no(c, 9, DRUM_HH, h65); no(c, 9, DRUM_RIM, sc(v, 90)); }
                _ => {}
            }
        } else if on_eighth { no(c, 9, DRUM_HH, h40); }
        PAT_JAZZ => {
            let b = beat % 8;
            if on_beat {
                match b {
                    0 | 2 | 6 => { no(c, 9, DRUM_RIDE, h60); }
                    4 => { no(c, 9, DRUM_RIDE, h60); no(c, 9, 44, sc(v, 40)); }
                    7 => { no(c, 9, DRUM_RIDE, h60); no(c, 9, 44, sc(v, 40)); no(c, 9, DRUM_RIM, sc(v, 50)); }
                    _ => {}
                }
            } else if on_eighth { no(c, 9, DRUM_HH, 35); }
        }
        PAT_POP => {
            let b = beat % 8;
            if on_beat {
                match b {
                    0 => { no(c, 9, DRUM_KICK, sc(v, 85)); no(c, 9, DRUM_HH, sc(v, 50)); }
                    2 => { no(c, 9, DRUM_SNARE, sc(v, 70)); no(c, 9, DRUM_HH, sc(v, 50)); }
                    4 => { no(c, 9, DRUM_KICK, sc(v, 75)); no(c, 9, DRUM_HH, sc(v, 50)); }
                    6 => { no(c, 9, DRUM_SNARE, sc(v, 65)); no(c, 9, DRUM_HH, sc(v, 50)); }
                    _ => {}
                }
            } else if on_eighth { no(c, 9, DRUM_HH, sc(v, 45)); }
        }
        PAT_BOSSA => if on_beat {
            match b {
                0 => { no(c, 9, DRUM_KICK, sc(v, 55)); no(c, 9, DRUM_HH, h45); }
                1 => { no(c, 9, DRUM_SNARE, sc(v, 30)); no(c, 9, DRUM_HH, h45); }
                2 => { no(c, 9, DRUM_KICK, sc(v, 60)); no(c, 9, DRUM_HH, h45); }
                3 => { no(c, 9, DRUM_KICK, sc(v, 50)); no(c, 9, DRUM_HH, h45); }
                _ => {}
            }
        } else if on_eighth { no(c, 9, DRUM_HH, h40); }
        PAT_ONEDROP => if on_beat {
            match b {
                0 => { no(c, 9, DRUM_KICK, sc(v, 90)); no(c, 9, DRUM_HH, h55); }
                1 => { no(c, 9, DRUM_HH, h40); }
                2 => { no(c, 9, DRUM_KICK, sc(v, 90)); no(c, 9, DRUM_RIM, sc(v, 65)); no(c, 9, DRUM_HH, h45); }
                3 => { no(c, 9, DRUM_HH, h55); }
                _ => {}
            }
        } else if on_eighth { no(c, 9, DRUM_HH, h40); }
        _ => if on_beat {
            match b {
                0 => { no(c, 9, DRUM_KICK, sc(v, 90)); no(c, 9, DRUM_HH, hh); }
                1 => { no(c, 9, DRUM_SNARE, sc(v, 75)); no(c, 9, DRUM_HH, hh); }
                2 => { no(c, 9, DRUM_KICK, sc(v, 80)); no(c, 9, DRUM_HH, hh); }
                3 => { no(c, 9, DRUM_SNARE, sc(v, 70)); no(c, 9, DRUM_HH, hh); }
                _ => {}
            }
        } else if on_eighth { no(c, 9, DRUM_HH, h8); }
    }
}

// ─── Play notes ─────────────────────────────────────────────────────────
pub fn play_notes(c: &mut MidiOutputConnection, notes: &[String], mv: u8) {
    let mut v: Vec<u8> = vec![];
    for n in notes { if let Ok(m) = note_midi(n) { v.push(m) } }
    if v.is_empty() { return }
    rch(c);
    for &ch in &[0u8, 2, 3] { cc(c, ch, 101, 0); cc(c, ch, 100, 1); cc(c, ch, 6, 62); cc(c, ch, 38, 2) }
    pc(c, 0, 51); pc(c, 2, 33);
    for _ in 0..2 {
        for &n in &v {
            std::thread::sleep(Duration::from_millis(240));
            if n < MIN_NOTE { no_mv(c, 2, n, 35, mv) } else { no_mv(c, 0, n, 15, mv) }
        }
    }
    rch(c);
    println!("  notes: {v:?}");
}

// ─── Setup tracks ───────────────────────────────────────────────────────
fn setup_tracks(c: &mut MidiOutputConnection, lc: &Live) {
    for t in &lc.tracks {
        let ch = t.channel;
        if ch == 9 { pc(c, ch, 1); continue }
        pc(c, ch, t.program.load(Ordering::Relaxed) as u8);
    }
}

// ─── Apply tracks ───────────────────────────────────────────────────────
pub fn apply_tracks(lc: &Live, cfg: &[TrackCfg]) {
    for tc in cfg {
        if let Some(i) = lc.tracks.iter().position(|t| t.channel == tc.channel) {
            if let Some(p) = tc.program { lc.tracks[i].program.store(p, Ordering::Relaxed) }
            if let Some(v) = tc.volume { lc.tracks[i].volume.store(v, Ordering::Relaxed) }
            if let Some(m) = tc.mute { lc.tracks[i].mute.store(m, Ordering::Relaxed) }
        }
    }
}

// ─── Play sequence ──────────────────────────────────────────────────────
pub fn play_seq(c: &mut MidiOutputConnection, ev: &[ChordEv], lc: &Live, do_loop: bool) {
    loop {
        rch(c);
        setup_tracks(c, lc);
        std::thread::sleep(Duration::from_millis(2));

        let mut prev_nappe: Vec<u8> = vec![];
        let mut prev_lead: Vec<u8> = vec![];
        let mut prev_accent: Vec<u8> = vec![];
        let t_lead = &lc.tracks[TRACK_LEAD];
        let t_bass = &lc.tracks[TRACK_BASS];
        let t_str = &lc.tracks[TRACK_STR];
        let t_drums = &lc.tracks[TRACK_DRUMS];
        let ch_lead = t_lead.channel;
        let ch_bass = t_bass.channel;
        let ch_str = t_str.channel;
        let t_accent = &lc.tracks[TRACK_ACCENT];
        let ch_accent = t_accent.channel;
        let walking = lc.walking.load(Ordering::Relaxed);
        let mv = lc.master_vol.load(Ordering::Relaxed);
        let _loop_on = lc.use_loops.load(Ordering::Relaxed);
        let _l_off = lc.loop_offset.load(Ordering::Relaxed);
        let mut seed: u64 = 0;

        for (i, e) in ev.iter().enumerate() {
            let mut m: Vec<u8> = vec![];
            for n in &e.notes { if let Ok(x) = note_midi(n) { m.push(x) } }
            if m.is_empty() {
                if i > 0 { rch(c); cc(c, 9, 120, 0) }
                let dur = (60_000.0 / lc.tempo.load(Ordering::Relaxed).max(20) as f64 * e.beats) as u64;
                let start = std::time::Instant::now();
                let mut idx = 0u64;
                let mut last_b_drums = u64::MAX;
                let dur_f = dur as f64;
                while start.elapsed().as_secs_f64() * 1000.0 < dur_f && !lc.stop.load(Ordering::Relaxed) {
                    let tempo_f = lc.tempo.load(Ordering::Relaxed).max(20) as f64;
                    let bd_ms = 60_000.0 / tempo_f;
                    let delay_ms = (bd_ms / 4.0).max(30.0);
                    let target = start + Duration::from_secs_f64(idx as f64 * delay_ms / 1000.0);
                    let now = std::time::Instant::now();
                    if target > now { std::thread::sleep(target - now) }
                    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                    let pt = lc.pattern.load(Ordering::Relaxed);
                    let sig = lc.sig.load(Ordering::Relaxed);
                    let bars = (sig / 10).max(1) as u64;
                    let beat = (elapsed_ms / bd_ms) as u64;
                    if !t_drums.mute.load(Ordering::Relaxed) {
                        if last_b_drums == u64::MAX || beat > last_b_drums {
                            let dvol = sc(t_drums.volume.load(Ordering::Relaxed), mv);
                            drum_hit(c, beat, pt, true, false, bars, dvol, 127);
                            last_b_drums = beat;
                        }
                        let beat_pos = elapsed_ms % bd_ms;
                        if beat_pos > bd_ms / 2.0 - 10.0 && beat_pos < bd_ms / 2.0 + 10.0 {
                            let dvol = sc(t_drums.volume.load(Ordering::Relaxed), mv);
                            drum_hit(c, beat, pt, false, true, bars, dvol, 127);
                        }
                    }
                    idx += 1;
                }
                if lc.stop.load(Ordering::Relaxed) { break }
                continue;
            }
            if i > 0 { rch(c); cc(c, 9, 120, 0) }

            let root = m[0];
            let nappe_notes: Vec<u8> = m[1..].to_vec();
            let dur = (60_000.0 / lc.tempo.load(Ordering::Relaxed).max(20) as f64 * e.beats) as u64;
            let start = std::time::Instant::now();
            let mut idx = 0u64;
            let mut last_b_drums = u64::MAX;
            let mut last_b_bass = 0u64;
            let mut prev_bass_note: u8 = 0;

            let mut walking_notes: [u8; 4] = [root, root, root, root];
            if walking && PAT_REGGAE != lc.pattern.load(Ordering::Relaxed) {
                let next_root = if let Some(ne) = ev.get(i + 1) {
                    let nv = notes_from_vec(&ne.notes);
                    if !nv.is_empty() { nv[0] } else { root }
                } else {
                    if let Some(ne) = ev.get(0) {
                        let nv = notes_from_vec(&ne.notes);
                        if !nv.is_empty() { nv[0] } else { root }
                    } else { root }
                };
                walking_notes = generate_walking_bass(&m, next_root, seed, m.len() >= 2 && is_minor(&m[1..]));
                seed = seed.wrapping_add(1);
            }

            if !t_bass.mute.load(Ordering::Relaxed) {
                let bvol = sc(t_bass.volume.load(Ordering::Relaxed), mv);
                let bass_note = if walking { walking_notes[0] } else { root };
                no_mv(c, ch_bass, bass_note, bvol, mv);
                prev_bass_note = bass_note;
                last_b_bass = 0;
            }

            if !t_str.mute.load(Ordering::Relaxed) {
                for n in &prev_nappe { no(c, ch_str, *n, 0) }
                let str_vol = sc(t_str.volume.load(Ordering::Relaxed), mv);
                for n in &nappe_notes { no_mv(c, ch_str, *n, str_vol, mv) }
                prev_nappe = nappe_notes.clone();
            }

            let dur_f = dur as f64;
            while start.elapsed().as_secs_f64() * 1000.0 < dur_f && !lc.stop.load(Ordering::Relaxed) {
                let tempo_f = lc.tempo.load(Ordering::Relaxed).max(20) as f64;
                let bd_ms = 60_000.0 / tempo_f;
                let delay_ms = (bd_ms / 4.0).max(30.0);
                let target = start + Duration::from_secs_f64(idx as f64 * delay_ms / 1000.0);
                let now = std::time::Instant::now();
                if target > now { std::thread::sleep(target - now) }
                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                let pt = lc.pattern.load(Ordering::Relaxed);
                let sig = lc.sig.load(Ordering::Relaxed);
                let bars = (sig / 10).max(1) as u64;
                let beat = (elapsed_ms / bd_ms) as u64;

                if !t_drums.mute.load(Ordering::Relaxed) {
                    if last_b_drums == u64::MAX || beat > last_b_drums {
                        let dvol = sc(t_drums.volume.load(Ordering::Relaxed), mv);
                        drum_hit(c, beat, pt, true, false, bars, dvol, 127);
                        last_b_drums = beat;
                    }
                    let beat_pos = elapsed_ms % bd_ms;
                    if beat_pos > bd_ms / 2.0 - 10.0 && beat_pos < bd_ms / 2.0 + 10.0 {
                        let dvol = sc(t_drums.volume.load(Ordering::Relaxed), mv);
                        drum_hit(c, beat, pt, false, true, bars, dvol, 127);
                    }
                }

                if !t_bass.mute.load(Ordering::Relaxed) {
                    if beat > last_b_bass {
                        let bvol = sc(t_bass.volume.load(Ordering::Relaxed), mv);
                        let bass_note = if walking {
                            let bi = (beat % 4) as usize;
                            walking_notes[bi]
                        } else { root };
                        no(c, ch_bass, prev_bass_note, 0);
                        no_mv(c, ch_bass, bass_note, bvol, mv);
                        prev_bass_note = bass_note;
                        last_b_bass = beat;
                    }
                }

                if !m.is_empty() {
                    let lead_mute = t_lead.mute.load(Ordering::Relaxed);
                    if idx % 4 == 3 && !prev_lead.is_empty() {
                        for &n in &prev_lead { no(c, ch_lead, n, 0) }
                        prev_lead.clear();
                    }
                    if idx % 4 == 2 && !lead_mute {
                        let lvol = sc(t_lead.volume.load(Ordering::Relaxed), mv);
                        prev_lead = m.clone();
                        for &note in &m { no_mv(c, ch_lead, note, lvol, mv) }
                    }
                }

                if !m.is_empty() {
                    let accent_mute = t_accent.mute.load(Ordering::Relaxed);
                    if idx % 8 == 5 && !prev_accent.is_empty() {
                        for &n in &prev_accent { no(c, ch_accent, n, 0) }
                        prev_accent.clear();
                    }
                    if idx % 8 == 4 && !accent_mute {
                        let avol = sc(t_accent.volume.load(Ordering::Relaxed), mv);
                        prev_accent = m.clone();
                        for &note in &m { no_mv(c, ch_accent, note, avol, mv) }
                    }
                }

                idx += 1;
            }
            if lc.stop.load(Ordering::Relaxed) { break }
        }
        for n in &prev_nappe { no(c, ch_str, *n, 0) }
        for n in &prev_lead { no(c, ch_lead, *n, 0) }
        for n in &prev_accent { no(c, ch_accent, *n, 0) }
        if lc.stop.load(Ordering::Relaxed) || !do_loop { break }
    }
    rch(c);
    println!("  done ({} evts)", ev.len());
}

// ─── TrackCfg (partagé avec main.rs) ─────────────────────────────────────
#[derive(Clone, Deserialize)]
pub struct TrackCfg {
    pub channel: u8,
    pub program: Option<u16>,
    pub volume: Option<u8>,
    pub mute: Option<bool>,
}

// ─── ChordEv (partagé avec main.rs) ──────────────────────────────────────
#[derive(Clone, Deserialize)]
pub struct ChordEv {
    pub notes: Vec<String>,
    pub beats: f64,
}
