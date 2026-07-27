/// Sample Bank — charge des fichiers WAV nommés `snap_BPM.wav` depuis
/// `~/samples/drums/` et les joue en temps réel via rodio.
///
/// Convention de nommage :
///   kick_120.wav     → sample "kick" pour 120 BPM
///   snare_120.wav    → sample "snare" pour 120 BPM
///   hh_140.wav       → hi-hat sample pour 140 BPM
///   rimshot_120.wav  → rimshot (mapping → MIDI 37)
///
/// Mapping snap → MIDI note (pour chercher le bon sample depuis drum_hit).
/// Si un sample existe pour (snap, tempo) on le joue, sinon on retourne
/// false pour que l'appelant utilise le MIDI classique.
use rodio::{OutputStream, Sink};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Mutex, OnceLock};

const DRUM_DIR: &str = "/home/legoeland/samples/drums";

static BANK: OnceLock<Mutex<SampleBank>> = OnceLock::new();

// ─── État global (sans bloquer le mutex du bank pour les flags) ─────────
static USE_SAMPLES: AtomicBool = AtomicBool::new(false);
static CUR_TEMPO: AtomicU16 = AtomicU16::new(120);

// ─── Data ───────────────────────────────────────────────────────────────
pub struct SampleBank {
    sink: Sink,
    /// tempo → (snap_name → pcm mono f32)
    samples: HashMap<u16, HashMap<String, Vec<f32>>>,
    sample_rate: u32,
}

/// Mapping snap → MIDI note (minuscule sans extension)
fn snap_to_note(snap: &str) -> Option<u8> {
    match snap.to_lowercase().as_str() {
        "kick" => Some(36),
        "snare" => Some(38),
        "rim" | "rimshot" => Some(37),
        "hh" | "hihat" | "hi-hat" | "cc" => Some(42),
        "hh_open" | "hhopen" | "oh" => Some(46),
        "ride" => Some(51),
        "crash" => Some(49),
        "tom_hi" => Some(48),
        "tom_mid" => Some(45),
        "tom_lo" => Some(41),
        "clap" => Some(39),
        _ => None,
    }
}

/// MIDI note → snap name préféré
fn note_to_snap(note: u8) -> Option<&'static str> {
    match note {
        36 => Some("kick"),
        38 => Some("snare"),
        37 => Some("rim"),
        42 => Some("hh"),
        46 => Some("hh_open"),
        51 => Some("ride"),
        49 => Some("crash"),
        48 => Some("tom_hi"),
        45 => Some("tom_mid"),
        41 => Some("tom_lo"),
        39 => Some("clap"),
        _ => None,
    }
}

impl SampleBank {
    fn scan_dir() -> Self {
        let (_stream, handle) = OutputStream::try_default()
            .expect("rodio: impossible d'ouvrir la sortie audio");
        let sink = Sink::try_new(&handle)
            .expect("rodio: impossible de créer le sink");
        let mut samples: HashMap<u16, HashMap<String, Vec<f32>>> = HashMap::new();
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

                // Parse "snap_BPM" — dernier underscore sépare snap et bpm
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
                        eprintln!(
                            "  ⚠️  {}: '{:?}' n'est pas un BPM valide",
                            fname,
                            &fname[us + 1..]
                        );
                        continue;
                    }
                };
                let snap = fname[..us].to_string();

                // Charger le WAV
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
                println!("   ✅ {} ({} bpm, snap='{}', {}s)", fname, bpm, snap, dur_s);
                samples.entry(bpm).or_default().insert(snap, pcm);
                count += 1;
            }
        }

        if count == 0 {
            println!("   ℹ️  Aucun sample trouvé dans {}", DRUM_DIR);
            println!("   └─ Nommez vos fichiers snap_BPM.wav (ex: kick_120.wav)");
        } else {
            println!("   {} samples chargés", count);
        }

        SampleBank {
            sink,
            samples,
            sample_rate,
        }
    }

    /// Joue le sample correspondant à `note` au tempo `tempo`.
    /// Retourne true si un sample a été trouvé et joué.
    fn play_note(&self, note: u8, velocity: u8, tempo: u16) -> bool {
        let snap = match note_to_snap(note) {
            Some(s) => s,
            None => return false,
        };

        // Essayer d'abord le tempo exact
        if let Some(bucket) = self.samples.get(&tempo) {
            if let Some(pcm) = bucket.get(snap) {
                let vol = (velocity as f32 / 127.0).min(1.0);
                let scaled: Vec<f32> = pcm.iter().map(|&s| s * vol).collect();
                self.sink
                    .append(rodio::buffer::SamplesBuffer::new(1, self.sample_rate, scaled));
                return true;
            }
        }

        // Fallback : chercher le même snap à n'importe quel tempo
        for (_t, bucket) in &self.samples {
            if let Some(pcm) = bucket.get(snap) {
                let vol = (velocity as f32 / 127.0).min(1.0);
                let scaled: Vec<f32> = pcm.iter().map(|&s| s * vol).collect();
                self.sink
                    .append(rodio::buffer::SamplesBuffer::new(1, self.sample_rate, scaled));
                return true;
            }
        }

        false
    }

    /// Retourne la liste des tempos disponibles et leurs snap names
    fn list_available(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        let mut tempos: Vec<u16> = self.samples.keys().copied().collect();
        tempos.sort();
        for t in tempos {
            let bucket = &self.samples[&t];
            let mut snaps: Vec<String> = bucket.keys().cloned().collect();
            snaps.sort();
            map.insert(
                t.to_string(),
                serde_json::Value::Array(snaps.into_iter().map(serde_json::Value::String).collect()),
            );
        }
        serde_json::Value::Object(map)
    }
}

// ─── API publique ───────────────────────────────────────────────────────

pub fn init() {
    let bank = SampleBank::scan_dir();
    BANK.set(Mutex::new(bank)).unwrap_or_else(|_| {
        eprintln!("⚠️  SampleBank déjà initialisée");
    });
}

pub fn set_use_samples(enabled: bool) {
    USE_SAMPLES.store(enabled, Ordering::Relaxed);
    if enabled {
        println!("  🎛️  Sample drums activé");
    } else {
        println!("  🎛️  Sample drums désactivé (MIDI)");
    }
}

pub fn set_current_tempo(tempo: u16) {
    CUR_TEMPO.store(tempo, Ordering::Relaxed);
}

pub fn is_active() -> bool {
    USE_SAMPLES.load(Ordering::Relaxed) && BANK.get().is_some()
}

/// Joue le sample pour `note` à la vélocité `velocity`.
/// Retourne true si un sample a été trouvé et joué, false pour fallback MIDI.
pub fn play_drum(note: u8, velocity: u8) -> bool {
    if !USE_SAMPLES.load(Ordering::Relaxed) {
        return false;
    }
    let tempo = CUR_TEMPO.load(Ordering::Relaxed);
    if let Some(mtx) = BANK.get() {
        if let Ok(bank) = mtx.lock() {
            return bank.play_note(note, velocity, tempo);
        }
    }
    false
}

/// Stop tous les sons en cours
pub fn stop_all() {
    if let Some(mtx) = BANK.get() {
        if let Ok(bank) = mtx.lock() {
            bank.sink.stop();
        }
    }
}

/// Retourne la liste des échantillons disponibles (pour l'API)
pub fn get_available() -> serde_json::Value {
    if let Some(mtx) = BANK.get() {
        if let Ok(bank) = mtx.lock() {
            return bank.list_available();
        }
    }
    serde_json::Value::Null
}

/// Retourne true si des samples existent pour `tempo`
pub fn has_for_tempo(tempo: u16) -> bool {
    if let Some(mtx) = BANK.get() {
        if let Ok(bank) = mtx.lock() {
            return bank.samples.contains_key(&tempo);
        }
    }
    false
}
