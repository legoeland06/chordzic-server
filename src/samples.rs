use rodio::{OutputStream, Sink};
use std::collections::HashMap;
use std::fs::File;
use std::sync::{Mutex, OnceLock};

const SAMPLES_DIR: &str = "/home/legoeland/samples";

static PLAYER: OnceLock<Mutex<SamplePlayer>> = OnceLock::new();

struct SamplePlayer {
    sink: Sink,
    buffers: HashMap<u8, Vec<f32>>,
}

pub fn init() {
    let (_stream, handle) = match OutputStream::try_default() {
        Ok(s) => s,
        Err(e) => { eprintln!("⚠️ Audio: {e}"); return; }
    };
    let sink = match Sink::try_new(&handle) {
        Ok(s) => s,
        Err(e) => { eprintln!("⚠️ Sink: {e}"); return; }
    };

    let mut buffers = HashMap::new();
    let samples = [
        (36u8, "kick.wav"),
        (38, "snare.wav"),
        (37, "Rimshot.wav"),
        (42, "cc.wav"),
        (44, "cc.wav"),
        (51, "cc.wav"),
    ];

    for (note, name) in &samples {
        let path = format!("{}/{}", SAMPLES_DIR, name);
        if let Ok(file) = File::open(&path) {
            if let Ok(reader) = hound::WavReader::new(file) {
                let spec = reader.spec();
                let sr = spec.sample_rate;
                let data: Vec<i16> = reader.into_samples::<i16>().filter_map(|s| s.ok()).collect();
                let pcm: Vec<f32> = if spec.channels == 2 {
                    data.chunks(2).map(|c| (c[0] as f32 + c[1] as f32) / 65536.0).collect()
                } else {
                    data.iter().map(|&s| s as f32 / 32768.0).collect()
                };
                println!("   ✅ {} (note {}, {}hz, {}s)", name, note, sr, pcm.len() as f32 / sr as f32);
                buffers.insert(*note, pcm);
            }
        } else {
            eprintln!("   ⚠️ {} introuvable", path);
        }
    }

    let n = buffers.len();
    let player = SamplePlayer { sink, buffers };
    PLAYER.set(Mutex::new(player)).ok();
    std::mem::forget(_stream);
    println!("   {} samples charges", n);
}

pub fn play(note: u8, velocity: u8) {
    if let Some(mtx) = PLAYER.get() {
        if let Ok(p) = mtx.lock() {
            if let Some(buf) = p.buffers.get(&note) {
                let vol = (velocity as f32 / 127.0).min(1.0);
                let scaled: Vec<f32> = buf.iter().map(|&s| s * vol).collect();
                p.sink.append(rodio::buffer::SamplesBuffer::new(1, 44100, scaled));
            }
        }
    }
}

pub fn stop_all() {
    if let Some(mtx) = PLAYER.get() {
        if let Ok(p) = mtx.lock() {
            p.sink.stop();
        }
    }
}

pub fn is_ready() -> bool {
    PLAYER.get().is_some()
}
