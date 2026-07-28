/// Loop Player — charge des fichiers WAV nommés `nom_BPM.wav` depuis
/// `~/samples/drums/` et les joue en boucle via rodio, en simultané
/// avec la séquence d'accords.
///
/// Convention de nommage :
///   pattern_120.wav   → loop "pattern" pour 120 BPM
///
/// Le sink rodio est recréé à chaque play/stop pour éviter tout état
/// résiduel (sink.stop() le détache définitivement du mixeur).
use rodio::{OutputStream, OutputStreamHandle, Sink, Source as RodioSource};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

const DRUM_DIR: &str = "/home/legoeland/samples/drums";

static BANK: OnceLock<Mutex<LoopPlayer>> = OnceLock::new();

static USE_LOOPS: AtomicBool = AtomicBool::new(false);

// ─── Data ───────────────────────────────────────────────────────────────
pub struct LoopPlayer {
    handle: OutputStreamHandle,
    sink: Option<Sink>,
    loops: HashMap<u16, HashMap<String, Vec<f32>>>,
    sample_rate: u32,
    last_tempo: u16,
    last_name: String,
}

impl LoopPlayer {
    fn scan_dir() -> Self {
        let (_stream, handle) = OutputStream::try_default()
            .expect("rodio: impossible d'ouvrir la sortie audio");
        std::mem::forget(_stream);

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
        } else {
            println!("   {} loops chargés", count);
        }

        LoopPlayer {
            handle,
            sink: None,
            loops,
            sample_rate,
            last_tempo: 0,
            last_name: String::new(),
        }
    }

    /// Ré-échantillonne linéairement `data` vers `target_len` samples.
    fn resample(data: &[f32], target_len: usize) -> Vec<f32> {
        let len = data.len();
        if len == 0 || target_len == 0 || len == target_len {
            return data.to_vec();
        }
        let ratio = len as f64 / target_len as f64;
        let mut out = Vec::with_capacity(target_len);
        for i in 0..target_len {
            let pos = i as f64 * ratio;
            let idx = pos as usize;
            let frac = pos - idx as f64;
            let val = if idx + 1 < len {
                data[idx] as f64 * (1.0 - frac) + data[idx + 1] as f64 * frac
            } else {
                data[idx] as f64
            };
            out.push(val as f32);
        }
        out
    }

    /// Démarre la boucle — crée un NOUVEAU sink
    /// Le WAV est resamplé pour que sa durée corresponde exactement à 4, 8, 12…
    /// temps au BPM donné, évitant toute dérive entre répétitions.
    fn start(&mut self, tempo: u16, name: Option<&str>, offset_ms: i32, total_beats: f64) {
        self.sink = None;

        let bucket = match self.loops.get(&tempo) {
            Some(b) => b,
            None => return,
        };

        let pcm = match name.and_then(|n| bucket.get(n)).or_else(|| bucket.values().next()) {
            Some(p) => p,
            None => return,
        };

        // 1. Resampler le WAV pour qu'il fasse exactement `total_beats` temps
        let target_samples =
            (total_beats * 60.0 / tempo as f64 * self.sample_rate as f64) as usize;
        let resampled = Self::resample(pcm, target_samples.max(1));

        // 2. Décalage cyclique (préserve la durée exacte)
        let len = resampled.len();
        let offset = if len > 0 && offset_ms != 0 {
            let samp = (offset_ms.abs() as usize * self.sample_rate as usize / 1000) % len;
            if offset_ms > 0 { samp } else { len - samp }
        } else {
            0
        };

        let vol = LOOP_VOLUME.load(Ordering::Relaxed) as f32 / 127.0;
        let final_pcm: Vec<f32> = if offset > 0 && offset < len {
            let (tail, head) = resampled.split_at(offset);
            [head, tail].concat().into_iter().map(|s| s * vol).collect()
        } else {
            resampled.into_iter().map(|s| s * vol).collect()
        };

        if final_pcm.is_empty() {
            return;
        }

        let source = rodio::buffer::SamplesBuffer::new(1, self.sample_rate, final_pcm)
            .repeat_infinite();

        self.last_tempo = tempo;
        self.last_name = name.unwrap_or("").to_string();

        match Sink::try_new(&self.handle) {
            Ok(s) => {
                s.append(source);
                self.sink = Some(s);
                println!(
                    "  🔁 Loop '{}' à {} bpm ({} éch → {} éch, {}m battements, offset {}ms)",
                    self.last_name,
                    tempo,
                    pcm.len(),
                    len,
                    total_beats,
                    offset_ms
                );
            }
            Err(e) => eprintln!("  ⚠️ Erreur création sink: {}", e),
        }
    }

    /// Relance la boucle avec un nouvel offset (appelé quand l'utilisateur tourne le spinner)
    fn restart_offset(&mut self, offset_ms: i32) {
        if self.sink.is_none() || self.last_tempo == 0 {
            return;
        }
        let tempo = self.last_tempo;
        let name = self.last_name.clone();
        let name_opt = if name.is_empty() { None } else { Some(name.as_str()) };
        self.start(tempo, name_opt, offset_ms, 4.0);
    }

    /// Arrête la boucle — détruit le sink (le drop stoppe le son)
    fn stop(&mut self) {
        if self.sink.is_some() {
            self.sink = None;
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

pub fn set_current_tempo(_tempo: u16) { /* le bank n'a pas besoin de savoir, le lookup se fait par tempo */ }

static LOOP_VOLUME: AtomicU8 = AtomicU8::new(80);

pub fn set_volume(vol: u8) {
    LOOP_VOLUME.store(vol, Ordering::Relaxed);
}

pub fn set_use_loops(enabled: bool) {
    USE_LOOPS.store(enabled, Ordering::Relaxed);
    if !enabled {
        stop_loop();
    }
    println!("  🎛️  Loop {}", if enabled { "activé" } else { "désactivé" });
}

pub fn play_loop(tempo: u16, name: Option<&str>, offset_ms: i32, total_beats: f64) {
    if !USE_LOOPS.load(Ordering::Relaxed) {
        return;
    }
    if let Some(mtx) = BANK.get() {
        if let Ok(mut p) = mtx.lock() {
            if p.has_loop(tempo) {
                p.start(tempo, name, offset_ms, total_beats);
            }
        }
    }
}

pub fn stop_loop() {
    if let Some(mtx) = BANK.get() {
        if let Ok(mut p) = mtx.lock() {
            p.stop();
        }
    }
}

/// Met à jour l'offset en temps réel (relance la boucle si elle tourne)
pub fn update_offset(offset_ms: i32) {
    if !USE_LOOPS.load(Ordering::Relaxed) {
        return;
    }
    if let Some(mtx) = BANK.get() {
        if let Ok(mut p) = mtx.lock() {
            p.restart_offset(offset_ms);
        }
    }
}

pub fn get_available() -> serde_json::Value {
    if let Some(mtx) = BANK.get() {
        if let Ok(p) = mtx.lock() {
            return p.list_available();
        }
    }
    serde_json::Value::Null
}
