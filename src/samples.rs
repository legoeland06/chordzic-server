/// Loop Player — charge des fichiers WAV nommés `nom_BPM.wav` depuis
/// `~/samples/drums/` et les joue en boucle via rodio, en simultané
/// avec la séquence d'accords.
///
/// Convention de nommage :
///   reggae_120.wav   → loop "reggae" pour 120 BPM
///   rock_140.wav     → loop "rock" pour 140 BPM
///
/// Un spinner de décalage (offset_ms) permet d'ajuster le premier temps
/// du WAV par rapport au début des accords.
use rodio::{OutputStream, Sink, Source as RodioSource};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Mutex, OnceLock};


const DRUM_DIR: &str = "/home/legoeland/samples/drums";

static BANK: OnceLock<Mutex<LoopPlayer>> = OnceLock::new();

// ─── État global ────────────────────────────────────────────────────────
static USE_LOOPS: AtomicBool = AtomicBool::new(false);
static CUR_TEMPO: AtomicU16 = AtomicU16::new(120);

// ─── Data ───────────────────────────────────────────────────────────────
pub struct LoopPlayer {
    sink: Sink,
    /// tempo → (nom → pcm mono f32)
    loops: HashMap<u16, HashMap<String, Vec<f32>>>,
    sample_rate: u32,
    /// tempo actuellement en lecture (0 = aucun)
    playing_tempo: u16,
}

impl LoopPlayer {
    fn scan_dir() -> Self {
        let (_stream, handle) = OutputStream::try_default()
            .expect("rodio: impossible d'ouvrir la sortie audio");
        std::mem::forget(_stream);
        let sink = Sink::try_new(&handle)
            .expect("rodio: impossible de créer le sink");

        let mut loops: HashMap<u16, HashMap<String, Vec<f32>>> = HashMap::new();
        let mut sample_rate = 44100u32;

        let dir = Path::new(DRUM_DIR);
        if !dir.exists() {
            println!("   📁 {} n'existe pas, création", DRUM_DIR);
            let _ = std::fs::create_dir_all(dir);
        }

        let mut count = 0usize;
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("wav") {
                    continue;
                }
                let fname = match path.file_stem().and_then(|s| s.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };

                // Parse "nom_BPM" — dernier underscore sépare nom et bpm
                let us = match fname.rfind('_') {
                    Some(p) => p,
                    None => {
                        eprintln!("  ⚠️  {}: pas de '_', ignoré", fname);
                        continue;
                    }
                };
                let bpm: u16 = match fname[us + 1..].parse() {
                    Ok(b) => b,
                    Err(_) => {
                        eprintln!("  ⚠️  {}: BPM invalide", fname);
                        continue;
                    }
                };
                let snap = fname[..us].to_string();

                let reader = match hound::WavReader::open(&path) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("  ⚠️  {}: erreur hound: {}", fname, e);
                        continue;
                    }
                };
                let spec = reader.spec();
                sample_rate = spec.sample_rate;
                let data: Vec<i16> = reader
                    .into_samples::<i16>()
                    .filter_map(|s| s.ok())
                    .collect();
                let pcm: Vec<f32> = if spec.channels == 2 {
                    data.chunks(2)
                        .map(|c| (c[0] as f32 + c[1] as f32) / 65536.0)
                        .collect()
                } else {
                    data.iter().map(|&s| s as f32 / 32768.0).collect()
                };

                let dur_s = pcm.len() as f32 / sample_rate as f32;
                println!("   ✅ {} ({} bpm, '{}', {:.1}s)", fname, bpm, snap, dur_s);
                loops.entry(bpm).or_default().insert(snap, pcm);
                count += 1;
            }
        }

        if count == 0 {
            println!("   ℹ️  Aucun loop trouvé dans {}", DRUM_DIR);
            println!("   └─ Nommez vos fichiers nom_BPM.wav (ex: pattern_120.wav)");
        } else {
            println!("   {} loops chargés", count);
        }

        LoopPlayer {
            sink,
            loops,
            sample_rate,
            playing_tempo: 0,
        }
    }

    /// Démarre la boucle pour `name` à `tempo` avec un décalage de `offset_ms`.
    /// Si `name` est vide, prend le premier loop disponible pour ce tempo.
    fn start(&mut self, tempo: u16, name: Option<&str>, offset_ms: i32) {
        self.stop();

        let bucket = match self.loops.get(&tempo) {
            Some(b) => b,
            None => return,
        };

        let pcm = match name {
            Some(n) => bucket.get(n),
            None => bucket.values().next(),
        };

        let pcm = match pcm {
            Some(p) => p,
            None => return,
        };

        // Appliquer l'offset : ignorer les premières samples
        let skip_samples = if offset_ms > 0 {
            ((offset_ms as f64 / 1000.0) * self.sample_rate as f64) as usize
        } else {
            0
        };

        let data = if skip_samples > 0 && skip_samples < pcm.len() {
            &pcm[skip_samples..]
        } else {
            pcm.as_slice()
        };

        if data.is_empty() {
            return;
        }

        let scaled: Vec<f32> = data.to_vec(); // copie pour le sink
        let source =
            rodio::buffer::SamplesBuffer::new(1, self.sample_rate, scaled).repeat_infinite();
        self.sink.append(source);
        self.playing_tempo = tempo;
        println!("  🔁 Loop '{}' à {} bpm (offset {}ms)", name.unwrap_or("?"), tempo, offset_ms);
    }

    fn stop(&mut self) {
        if self.playing_tempo != 0 {
            self.sink.stop();
            self.sink.clear();
            self.playing_tempo = 0;
            println!("  ⏹ Loop arrêté");
        }
    }

    fn has_loop(&self, tempo: u16) -> bool {
        self.loops.contains_key(&tempo)
    }

    fn list_available(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        let mut tempos: Vec<u16> = self.loops.keys().copied().collect();
        tempos.sort();
        for t in tempos {
            let bucket = &self.loops[&t];
            let mut snaps: Vec<String> = bucket.keys().cloned().collect();
            snaps.sort();
            map.insert(
                t.to_string(),
                serde_json::Value::Array(
                    snaps.into_iter().map(serde_json::Value::String).collect(),
                ),
            );
        }
        serde_json::Value::Object(map)
    }
}

// ─── API publique ───────────────────────────────────────────────────────

pub fn init() {
    let player = LoopPlayer::scan_dir();
    BANK.set(Mutex::new(player)).unwrap_or_else(|_| {
        eprintln!("⚠️  LoopPlayer déjà initialisé");
    });
}

pub fn set_use_loops(enabled: bool) {
    USE_LOOPS.store(enabled, Ordering::Relaxed);
    if !enabled {
        if let Some(mtx) = BANK.get() {
            if let Ok(mut p) = mtx.lock() {
                p.stop();
            }
        }
    }
    println!("  🎛️  Loop drums {}", if enabled { "activé" } else { "désactivé" });
}

pub fn set_current_tempo(tempo: u16) {
    CUR_TEMPO.store(tempo, Ordering::Relaxed);
}

/// Démarre la boucle. Appelé par play_seq quand use_loops est true.
pub fn play_loop(tempo: u16, name: Option<&str>, offset_ms: i32) {
    if !USE_LOOPS.load(Ordering::Relaxed) {
        return;
    }
    if let Some(mtx) = BANK.get() {
        if let Ok(mut p) = mtx.lock() {
            if p.has_loop(tempo) {
                p.start(tempo, name, offset_ms);
            }
        }
    }
}

/// Arrête la boucle. Appelé par stop et à la fin de play_seq.
pub fn stop_loop() {
    if let Some(mtx) = BANK.get() {
        if let Ok(mut p) = mtx.lock() {
            p.stop();
        }
    }
}

pub fn has_loop_for(tempo: u16) -> bool {
    if let Some(mtx) = BANK.get() {
        if let Ok(p) = mtx.lock() {
            return p.has_loop(tempo);
        }
    }
    false
}

pub fn get_available() -> serde_json::Value {
    if let Some(mtx) = BANK.get() {
        if let Ok(p) = mtx.lock() {
            return p.list_available();
        }
    }
    serde_json::Value::Null
}
