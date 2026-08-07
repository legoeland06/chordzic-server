// ─── Piste de clic avec sortie audio DÉDIÉE (cpal) ─────────────────────────
//
// Principe :
//  - `play_seq` (moteur live) envoie un `Tick { accent, at }` à chaque
//    temps, avec `at` = instant exact (Instant) du temps, calculé sur la MÊME
//    horloge que les notes MIDI (target = start + idx*delay_ms).
//  - Un thread audio dédié reçoit ces ticks, les synthétise (sinus + decay)
//    et les joue sur un device cpal DISTINCT de la sortie principale
//    (ex : hub USB-C → 2e sortie casque) → le clic est audible à part,
//    calé sur le même tempo.
//  - `delay_ms` compense la latence relative des deux chemins audio
//    (MIDI→FluidSynth vs clic→cpal) : positif = clic retardé.
//
// Cross-platform : cpal gère CoreAudio (macOS), ALSA/Pulse (Linux),
// WASAPI (Windows) → aucun code spécifique par OS ici.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PI: f32 = std::f32::consts::PI;

// ─── État partagé (config du clic, visible du HTTP) ────────────────────────

pub struct ClickState {
    pub enabled: AtomicBool,
    /// Nom du device cpal (None = device par défaut)
    pub device: Mutex<Option<String>>,
    /// Volume 0-100
    pub volume: AtomicU8,
    /// Retard du clic en ms (positif = clic plus tard) — compensation latence
    pub delay_ms: AtomicI32,
    /// Accent sur le 1er temps de chaque mesure
    pub accent: AtomicBool,
}

impl Default for ClickState {
    fn default() -> Self {
        ClickState {
            enabled: AtomicBool::new(false),
            device: Mutex::new(None),
            volume: AtomicU8::new(80),
            delay_ms: AtomicI32::new(10),
            accent: AtomicBool::new(true),
        }
    }
}

// ─── Messages internes ─────────────────────────────────────────────────────

enum Msg {
    Tick { accent: bool, at: Instant },
    Ctrl(CtrlMsg),
}

enum CtrlMsg {
    SetEnabled(bool),
    SetDevice(Option<String>),
}

/// Sender clonable utilisé par le moteur live pour déclencher les ticks.
#[derive(Clone)]
pub struct ClickSender {
    tx: Sender<Msg>,
    pub state: Arc<ClickState>,
}

impl ClickSender {
    /// Envoie un tick si le clic est activé. `at` = instant du temps
    /// (même horloge que les notes MIDI).
    pub fn beat(&self, accent: bool, at: Instant) {
        if self.state.enabled.load(Ordering::Relaxed) {
            let _ = self.tx.send(Msg::Tick { accent, at });
        }
    }
}

/// Handle public (créé dans main) : état + contrôle + sender.
#[derive(Clone)]
pub struct ClickHandle {
    pub state: Arc<ClickState>,
    ctrl: Sender<Msg>,
    tick_tx: Sender<Msg>,
}

impl ClickHandle {
    pub fn sender(&self) -> ClickSender {
        ClickSender { tx: self.tick_tx.clone(), state: self.state.clone() }
    }
    pub fn set_enabled(&self, v: bool) {
        self.state.enabled.store(v, Ordering::Relaxed);
        let _ = self.ctrl.send(Msg::Ctrl(CtrlMsg::SetEnabled(v)));
    }
    pub fn set_device(&self, name: Option<String>) {
        *self.state.device.lock().unwrap() = name.clone();
        let _ = self.ctrl.send(Msg::Ctrl(CtrlMsg::SetDevice(name)));
    }
}

/// Démarre le thread audio du clic. Retourne le handle.
pub fn start_click(state: Arc<ClickState>) -> ClickHandle {
    let (tx, rx) = mpsc::channel::<Msg>();
    let st = state.clone();
    std::thread::spawn(move || click_audio_thread(rx, st));
    ClickHandle { state, ctrl: tx.clone(), tick_tx: tx }
}

// ─── Thread audio ──────────────────────────────────────────────────────────

struct AudioOut {
    _stream: cpal::Stream,
    ring: Arc<Mutex<VecDeque<f32>>>,
    sample_rate: f32,
}

fn open_output(device_name: &Option<String>) -> Option<AudioOut> {
    let host = cpal::default_host();
    let device = match device_name {
        Some(name) => {
            let found = host
                .output_devices()
                .ok()?
                .find(|d| {
                    d.name()
                        .map(|n| n.to_lowercase().contains(&name.to_lowercase()))
                        .unwrap_or(false)
                });
            match found {
                Some(d) => {
                    println!("   🎧 Clic → device « {} »", d.name().unwrap_or_default());
                    d
                }
                None => {
                    eprintln!("   ⚠️ Clic : device « {} » introuvable → défaut", name);
                    host.default_output_device()?
                }
            }
        }
        None => host.default_output_device()?,
    };

    let default_cfg = device.default_output_config().ok()?;
    let sample_rate = default_cfg.sample_rate().0 as f32;
    let channels = default_cfg.channels().min(2).max(1); // stéréo ou mono natif

    // Petit buffer (latence minimale) avec repli sur la config par défaut
    let ring: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
    let err_cb = |e| eprintln!("   ⚠️ Clic : erreur audio : {}", e);
    let build = |config: cpal::StreamConfig, ring: Arc<Mutex<VecDeque<f32>>>| {
        let ring_cb = ring.clone();
        device.build_output_stream(
            &config,
            move |data: &mut [f32], _| {
                let mut r = ring_cb.lock().unwrap();
                let ch = channels.max(1) as usize;
                for frame in data.chunks_mut(ch) {
                    let s = r.pop_front().unwrap_or(0.0);
                    for out in frame.iter_mut() {
                        *out = s;
                    }
                }
            },
            err_cb,
            None,
        )
    };
    let fast_cfg = cpal::StreamConfig {
        channels,
        sample_rate: default_cfg.sample_rate(),
        buffer_size: cpal::BufferSize::Fixed(128),
    };
    let stream = build(fast_cfg, ring.clone())
        .or_else(|_| build(default_cfg.into(), ring.clone()))
        .ok()?;

    stream.play().ok()?;
    Some(AudioOut { _stream: stream, ring, sample_rate })
}

/// Synthèse d'un tick : sinus avec decay exponentiel.
fn render_tick(accent: bool, volume: u8, sample_rate: f32) -> Vec<f32> {
    let freq = if accent { 2093.0 } else { 1568.0 }; // C7 / G6
    let dur = if accent { 0.045 } else { 0.035 };
    let n = (sample_rate * dur) as usize;
    let amp = (volume as f32 / 100.0).min(1.0) * 0.85;
    (0..n)
        .map(|i| {
            let t = i as f32 / sample_rate;
            let env = (-t * 40.0).exp();
            (t * freq * 2.0 * PI).sin() * env * amp
        })
        .collect()
}

fn click_audio_thread(rx: Receiver<Msg>, state: Arc<ClickState>) {
    let mut audio: Option<AudioOut> = None;
    let mut pending: VecDeque<(bool, Instant)> = VecDeque::new();
    let mut last_device: Option<String> = None;
    let mut last_enabled = state.enabled.load(Ordering::Relaxed);

    loop {
        // 1. Attendre le prochain message OU le prochain tick à jouer.
        //    Le timeout est borné par le tick le plus proche → réveil PRÉCIS
        //    à l'échéance (gigue < 1 ms), au lieu d'un polling lâche qui
        //    faisait varier l'instant de lecture du clic de ±15 ms.
        let delay = Duration::from_millis(state.delay_ms.load(Ordering::Relaxed).max(0) as u64);
        let timeout = match pending.front() {
            Some(&(_, at)) => {
                let until = at + delay;
                let t = until.saturating_duration_since(Instant::now());
                t.min(Duration::from_millis(15))
            }
            None => Duration::from_millis(15),
        };
        let msg = rx.recv_timeout(timeout);
        match msg {
            Ok(Msg::Tick { accent, at }) => pending.push_back((accent, at)),
            Ok(Msg::Ctrl(CtrlMsg::SetEnabled(v))) => {
                state.enabled.store(v, Ordering::Relaxed);
                last_enabled = v;
            }
            Ok(Msg::Ctrl(CtrlMsg::SetDevice(d))) => {
                last_device = d.clone();
                // reconstruit le stream au prochain passage
                audio.take();
            }
            Err(_) => {} // timeout
        }

        let enabled = last_enabled && state.enabled.load(Ordering::Relaxed);
        let device_name = last_device.clone();

        // 2. Gérer le cycle de vie du stream
        if enabled && audio.is_none() {
            audio = open_output(&device_name);
            if audio.is_none() {
                eprintln!("   ⚠️ Clic : impossible d'ouvrir une sortie audio");
            }
        } else if !enabled && audio.is_some() {
            audio.take();
        }

        // 3. Jouer les ticks arrivés à échéance
        if let Some(a) = audio.as_mut() {
            loop {
                let due = match pending.front() {
                    Some(&(_, at)) => at + delay <= Instant::now(),
                    None => break,
                };
                if !due {
                    break;
                }
                let (accent, _) = pending.pop_front().unwrap();
                let vol = state.volume.load(Ordering::Relaxed);
                let samples = render_tick(accent, vol, a.sample_rate);
                let mut r = a.ring.lock().unwrap();
                for s in samples {
                    r.push_back(s);
                }
            }
        } else {
            pending.clear();
        }
    }
}

// ─── Énumération des devices de sortie (pour le frontend) ──────────────────

#[derive(serde::Serialize)]
pub struct DeviceInfo {
    pub name: String,
    pub channels: u16,
}

pub fn list_output_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let mut out: Vec<DeviceInfo> = vec![];
    if let Ok(devices) = host.output_devices() {
        for d in devices {
            let name = d.name().unwrap_or_default();
            let channels = d.default_output_config().map(|c| c.channels()).unwrap_or(0);
            out.push(DeviceInfo { name, channels });
        }
    }
    out
}
