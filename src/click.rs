// ─── Piste de clic — MODE RENDU (mode Navig) ───────────────────────────────
//
// Deux façons d'utiliser le clic (au choix) :
//  - « Dans le rendu » (in_render) : le clic est MÉLANGÉ au WAV principal
//    (render::generate_click_smf + mix) → synchro échantillon-parfaite par
//    construction. Aucune autre config nécessaire.
//  - « Sortie dédiée » (out_device) : le serveur joue le clic (WAV rendu à
//    part) sur la sortie audio choisie (ex : hub USB-C) pendant que le
//    navigateur joue le son principal. Démarrage synchronisé par handshake
//    (start_in_ms), calage fin via delay_ms.
//
// Il n'y a PAS de clic live (mode MIDI temps réel) — retiré car
// désynchronisé (deux horloges audio indépendantes).

use cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, OutputStream, Sink};
use std::fs::File;
use std::io::BufReader;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

// Sons de clic disponibles (pour le rendu) :
pub const SOUND_GM_METRONOME: u8 = 0;
pub const SOUND_WOODBLOCK: u8 = 1;
pub const SOUND_AGOGO: u8 = 2;
pub const SOUND_TAIKO: u8 = 3;

pub struct ClickState {
    /// Volume 0-100
    pub volume: AtomicU8,
    /// Accent sur le 1er temps de chaque mesure
    pub accent: AtomicBool,
    /// Son du clic (SOUND_*)
    pub sound: AtomicU8,
    /// Clic MÉLANGÉ au WAV rendu (synchro parfaite) — source de vérité serveur
    pub in_render: AtomicBool,
    /// Sortie audio dédiée (None = pas de séparation)
    pub out_device: Mutex<Option<String>>,
    /// Ajustement du clic séparé en ms (calage à l'oreille)
    pub delay_ms: AtomicI32,
}

impl Default for ClickState {
    fn default() -> Self {
        ClickState {
            volume: AtomicU8::new(80),
            accent: AtomicBool::new(true),
            sound: AtomicU8::new(SOUND_GM_METRONOME),
            in_render: AtomicBool::new(false),
            out_device: Mutex::new(None),
            delay_ms: AtomicI32::new(0),
        }
    }
}

// ─── Lecture du clic séparé (rodio, sortie au choix) ───────────────────────

/// Sink courant (pour /navig-click-stop). Sink est Send + Sync.
static CURRENT_SINK: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();
fn current_sink() -> &'static Mutex<Option<Sink>> {
    CURRENT_SINK.get_or_init(|| Mutex::new(None))
}

/// Retrouve un device de sortie par nom (contient, insensible à la casse).
fn find_device(name: &str) -> Option<cpal::Device> {
    let host = cpal::default_host();
    let n = name.to_lowercase();
    host.output_devices().ok()?.find(|d| {
        d.name().map(|dn| dn.to_lowercase().contains(&n)).unwrap_or(false)
    })
}

/// Joue un WAV de clic sur la sortie choisie, après `start_in_ms` (+ delay_ms
/// de calage). Retourne immédiatement (lecture en arrière-plan).
pub fn play_click_wav(path: &str, device: Option<String>, start_in_ms: u64, delay_ms: i32) -> Result<(), String> {
    let path = path.to_string();
    std::thread::spawn(move || {
        // Attendre le démarrage synchronisé (+ calage utilisateur)
        let wait = start_in_ms as i64 + delay_ms as i64;
        if wait > 0 {
            std::thread::sleep(Duration::from_millis(wait as u64));
        }
        play_inner(&path, device.as_deref());
    });
    Ok(())
}

fn play_inner(path: &str, device: Option<&str>) {
    // Ouvrir la sortie (nommée ou défaut)
    let (_stream, handle) = match device {
        Some(name) => match find_device(name) {
            Some(d) => match OutputStream::try_from_device(&d) {
                Ok(s) => s,
                Err(_) => {
                    eprintln!("   ⚠️ Clic : sortie « {} » indisponible → défaut", name);
                    match OutputStream::try_default() {
                        Ok(s) => s,
                        Err(_) => return,
                    }
                }
            },
            None => {
                eprintln!("   ⚠️ Clic : sortie « {} » introuvable → défaut", name);
                match OutputStream::try_default() {
                    Ok(s) => s,
                    Err(_) => return,
                }
            }
        },
        None => match OutputStream::try_default() {
            Ok(s) => s,
            Err(_) => return,
        },
    };

    let sink = match Sink::try_new(&handle) {
        Ok(s) => s,
        Err(_) => return,
    };
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return,
    };
    let src = match Decoder::new(BufReader::new(file)) {
        Ok(s) => s,
        Err(_) => return,
    };
    sink.append(src);
    {
        let mut g = current_sink().lock().unwrap();
        *g = Some(sink);
    }
    println!("   🎧 Clic séparé joué sur « {} » ({})", device.unwrap_or("défaut"), path);

    // Attendre la fin de la lecture (ou l'arrêt via /navig-click-stop)
    loop {
        let done = {
            let g = current_sink().lock().unwrap();
            g.as_ref().map_or(true, |s| s.empty())
        };
        if done {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    {
        let mut g = current_sink().lock().unwrap();
        *g = None;
    }
}

/// Arrête le clic séparé en cours de lecture.
pub fn stop_click() {
    let mut g = current_sink().lock().unwrap();
    if let Some(s) = g.take() {
        s.stop();
        println!("   🛑 Clic séparé arrêté");
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

/// Nom du son courant (affichage frontend).
pub fn sound_name(sound: u8) -> &'static str {
    match sound {
        SOUND_WOODBLOCK => "Woodblock",
        SOUND_AGOGO => "Agogo",
        SOUND_TAIKO => "Taiko",
        _ => "Métronome GM",
    }
}

/// Charge l'état complet (pour GET /click).
pub struct ClickConfig {
    pub volume: u8,
    pub accent: bool,
    pub sound: u8,
    pub in_render: bool,
    pub out_device: Option<String>,
    pub delay_ms: i32,
}

pub fn load(state: &ClickState) -> ClickConfig {
    ClickConfig {
        volume: state.volume.load(Ordering::Relaxed),
        accent: state.accent.load(Ordering::Relaxed),
        sound: state.sound.load(Ordering::Relaxed),
        in_render: state.in_render.load(Ordering::Relaxed),
        out_device: state.out_device.lock().unwrap().clone(),
        delay_ms: state.delay_ms.load(Ordering::Relaxed),
    }
}
