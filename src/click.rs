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

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rodio::{Decoder, OutputStream, Sink};
use std::collections::VecDeque;
use std::fs::File;
use std::io::BufReader;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
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

/// Nombre de canaux de sortie d'un device (None si introuvable/illisible).
#[allow(dead_code)]
pub fn device_channels(name: &str) -> Option<u16> {
    let device = find_device(name)?;
    device.default_output_config().ok().map(|c| c.channels())
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

// ─── Lecture DOUBLE canaux (main ch1-2 + clic ch3-4) ──────────────────────
// Synchro ÉCHANTILLON-PARFAITE entre les deux sorties : UN SEUL flux cpal sur
// un appareil MULTICANAL (ex : Agrégat CoreAudio = sortie intégrée + hub USB-C),
// le main part sur les canaux 1-2, le clic sur les canaux 3-4. Une seule
// horloge → aucun décalage possible, c'est la seule façon musicalement correcte
// de sortir deux sons différents vers deux sorties physiques.

// ─── Lecture DOUBLE canaux (main ch1-2 + clic ch3-4) ──────────────────────
// Synchro ÉCHANTILLON-PARFAITE entre les deux sorties : UN SEUL flux cpal sur
// un appareil MULTICANAL (ex : Agrégat CoreAudio = sortie intégrée + hub USB-C),
// le main part sur les canaux 1-2, le clic sur les canaux 3-4. Une seule
// horloge → aucun décalage possible, c'est la seule façon musicalement correcte
// de sortir deux sons différents vers deux sorties physiques.

/// Flag d'arrêt partagé : le stream cpal n'est pas Send, il vit dans le
/// thread feeder ; l'arrêt passe donc par ce flag (le feeder drop le stream).
static DUAL_STOP: OnceLock<Arc<AtomicBool>> = OnceLock::new();
fn dual_stop_flag() -> &'static Arc<AtomicBool> {
    DUAL_STOP.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

/// cpal::Stream est marqué !Send par conservatisme, mais il est en pratique
/// portable entre threads pour ALSA/WASAPI/CoreAudio (le thread de callback
/// est interne à cpal ; le drop sur un autre thread est pris en charge).
struct SendStream(#[allow(dead_code)] cpal::Stream);
unsafe impl Send for SendStream {}

/// Lit un WAV 16-bit en Vec<i16> (tous canaux entrelacés).
fn read_wav_i16(path: &str) -> Result<(Vec<i16>, u16, u32), String> {
    let mut r = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = r.spec();
    let samples: Vec<i16> = r.samples::<i16>().collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    Ok((samples, spec.channels, spec.sample_rate))
}

/// Resampling linéaire i16 (toutes fréquences) — utilisé par play_dual pour
/// aligner les WAV (44,1 kHz) sur la fréquence du device (ex. 48 kHz) avant
/// lecture : cpal ne convertit PAS, un 1:1 jouerait le contenu trop vite
/// (tempo + pitch faussés de 48000/44100 ≈ 1,088×).
fn resample_i16(samples: &[i16], channels: u16, in_rate: u32, out_rate: u32) -> Vec<i16> {
    if in_rate == out_rate || samples.is_empty() {
        return samples.to_vec();
    }
    let ch = channels.max(1) as usize;
    let frames_in = samples.len() / ch;
    if frames_in == 0 {
        return samples.to_vec();
    }
    let frames_out = (frames_in as f64 * out_rate as f64 / in_rate as f64).round() as usize;
    let mut out = Vec::with_capacity(frames_out * ch);
    for f in 0..frames_out {
        let pos = f as f64 * in_rate as f64 / out_rate as f64;
        let i0 = pos.floor() as usize;
        let frac = pos - i0 as f64;
        let i1 = (i0 + 1).min(frames_in - 1);
        for c in 0..ch {
            let s0 = samples[i0 * ch + c] as f64;
            let s1 = samples[i1 * ch + c] as f64;
            let v = (s0 + (s1 - s0) * frac).round().clamp(-32768.0, 32767.0) as i16;
            out.push(v);
        }
    }
    out
}

/// Frame de départ de la lecture (offset en secondes × fréquence device).
/// Utilisé par play_dual pour démarrer main + clic à la position demandée
/// (scrub en mode séparé) : les deux WAV commencent au même offset → ils
/// restent échantillon-alignés.
fn playback_start_frame(start_sec: f64, sample_rate: u32) -> usize {
    if start_sec <= 0.0 {
        return 0;
    }
    (start_sec * sample_rate as f64).round() as usize
}

/// Frame suivante à jouer — gère la BOUCLE (repeat) : quand la position
/// atteint la fin (loop_end_frame), on repart à loop_start_frame si la
/// boucle est activée, sinon None (fin). Sans intervalle (loop_start=0,
/// loop_end=max_frames), le comportement est l'ancien : retour au début (0).
fn next_frame(
    i: usize,
    max_frames: usize,
    loop_start_frame: usize,
    loop_end_frame: usize,
    loop_playback: bool,
) -> Option<usize> {
    if i < loop_end_frame {
        return Some(i);
    }
    if loop_playback {
        if loop_end_frame > loop_start_frame && loop_start_frame < max_frames {
            return Some(loop_start_frame); // boucle sur l'intervalle [L, R[
        }
        if max_frames > 0 {
            return Some(0); // ancien comportement : boucle complète
        }
    }
    None
}

/// Joue main (canaux 1-2) + clic (canaux 3-4) sur l'appareil `device_name`
/// (doit avoir ≥ 4 canaux de sortie). Le clic est atténué par `click_gain`.
/// `start_sec` : offset de départ en secondes (les deux WAV démarrent à cet
/// offset, alignés) — 0 = depuis le début.
/// `loop_playback` : repeat — à la fin (ou à loop_end_sec), on repart à
/// loop_start_sec (ou 0 si pas d'intervalle).
pub fn play_dual(
    main_path: &str,
    click_path: &str,
    device_name: &str,
    state: Arc<ClickState>,
    start_sec: f64,
    loop_playback: bool,
    loop_start_sec: f64,
    loop_end_sec: f64,
) -> Result<(), String> {
    // Arrêter un éventuel lecteur précédent
    stop_dual();

    let device = find_device(device_name)
        .ok_or_else(|| format!("Sortie « {} » introuvable", device_name))?;
    let default_cfg = device.default_output_config().map_err(|e| e.to_string())?;
    // Le mode séparé exige 4 canaux : on force 4 (le device ALSA multi/agrégat
    // accepte ; sinon erreur → le serveur replie sur le clic mélangé).
    let channels: u16 = 4;

    let (main_s, main_ch, main_sr) = read_wav_i16(main_path)?;
    let (click_s, click_ch, click_sr) = read_wav_i16(click_path)?;
    if main_sr != click_sr {
        return Err(format!("Fréquences différentes : main {} Hz / clic {} Hz", main_sr, click_sr));
    }
    let sr = default_cfg.sample_rate().0;
    // ⚠️ RESAMPLING explicite vers la fréquence du device : cpal NE convertit
    // PAS (le commentaire « cpal convertit » était faux) — sans ça, un WAV
    // 44,1 kHz joué sur un device 48 kHz sort à 1,088× (tempo ET pitch
    // faussés). Bug corrigé : les deux WAV sont alignés sur `sr` avant
    // lecture, le délai/volume live restent calculés en frames device.
    let main_s = if sr == main_sr {
        main_s
    } else {
        eprintln!("   ℹ️ Clic : resampling main {} Hz → {} Hz (device)", main_sr, sr);
        resample_i16(&main_s, main_ch, main_sr, sr)
    };
    let click_s = if sr == click_sr {
        click_s
    } else {
        eprintln!("   ℹ️ Clic : resampling clic {} Hz → {} Hz (device)", click_sr, sr);
        resample_i16(&click_s, click_ch, click_sr, sr)
    };

    let stop = dual_stop_flag().clone();
    stop.store(false, Ordering::Relaxed);
    let ring: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
    let ring_cb = ring.clone();
    let err_cb = |e| eprintln!("   ⚠️ Dual : erreur audio : {}", e);
    let config = cpal::StreamConfig {
        channels,
        sample_rate: default_cfg.sample_rate(),
        buffer_size: cpal::BufferSize::Default,
    };
    let ch = channels.max(1) as usize;

    let stream = device
        .build_output_stream(
            &config,
            move |data: &mut [f32], _| {
                let mut r = ring_cb.lock().unwrap();
                for frame in data.chunks_mut(ch) {
                    for out in frame.iter_mut() {
                        *out = r.pop_front().unwrap_or(0.0);
                    }
                }
            },
            err_cb,
            None,
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;

    println!("   🎧 Lecture double canaux sur « {} » ({} ch) — main 1-2, clic 3-4", device_name, channels);

    // Thread alimenteur : possède le stream (via SendStream), interleave
    // main (ch1-2) + clic (ch3-4, gain), et lâche le stream à la fin.
    let stream = SendStream(stream);
    let ring_feed = ring.clone();
    std::thread::spawn(move || {
        let _stream = stream; // vit ici jusqu'à la fin de la lecture
        let mch = main_ch.max(1) as usize;
        let cch = click_ch.max(1) as usize;
        let max_frames = (main_s.len() / mch).max(click_s.len() / cch);
        let sr_us = sr.max(1) as usize;
        // Intervalle de boucle (locators) en frames device : la lecture
        // boucle [loop_start, loop_end[ au lieu de tout le buffer.
        let loop_start_frame = (loop_start_sec * sr as f64).round() as usize;
        let loop_end_frame = if loop_end_sec > 0.0 {
            ((loop_end_sec * sr as f64).round() as usize).min(max_frames)
        } else {
            max_frames
        };
        let loop_start_frame = loop_start_frame.min(loop_end_frame.saturating_sub(1));
        let mut i = playback_start_frame(start_sec, sr).min(max_frames);
        loop {
            // Frame courante — la boucle (repeat) repart au début (ou au
            // locator gauche) quand la fin (ou le locator droit) est atteinte.
            let frame = match next_frame(i, max_frames, loop_start_frame, loop_end_frame, loop_playback) {
                Some(f) => f,
                None => break,
            };
            i = frame;
            if stop.load(Ordering::Relaxed) {
                break;
            }
            // Compensation de latence LUE EN DIRECT dans l'état partagé :
            // modifier delay_ms/volume PENDANT la lecture décale le clic
            // immédiatement (calage à l'oreille sans relancer).
            //   delay_ms > 0 → retarde le CLIC ; < 0 → retarde le MAIN.
            let delay_ms = state.delay_ms.load(Ordering::Relaxed);
            let click_gain = state.volume.load(Ordering::Relaxed) as f32 / 100.0;
            let click_delay_frames = if delay_ms > 0 {
                (delay_ms as usize * sr_us) / 1000
            } else {
                0
            };
            let main_delay_frames = if delay_ms < 0 {
                ((-delay_ms) as usize * sr_us) / 1000
            } else {
                0
            };
            // Backpressure : garder ~200 ms d'avance max
            let frames_ready = {
                let r = ring_feed.lock().unwrap();
                r.len() / ch
            };
            if frames_ready > (sr / 5) as usize {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            let mi = if i >= main_delay_frames { i - main_delay_frames } else { usize::MAX };
            let ci = if i >= click_delay_frames { i - click_delay_frames } else { usize::MAX };
            let ml = main_s.get(mi * mch).copied().unwrap_or(0) as f32 / 32768.0;
            let mr = main_s.get(mi * mch + 1).copied().unwrap_or(0) as f32 / 32768.0;
            let cl = click_s.get(ci * cch).copied().unwrap_or(0) as f32 / 32768.0 * click_gain;
            let cr = click_s.get(ci * cch + 1).copied().unwrap_or(0) as f32 / 32768.0 * click_gain;
            let mut frame = vec![ml, mr, cl, cr];
            frame.resize(ch, 0.0);
            let mut r = ring_feed.lock().unwrap();
            for s in frame {
                r.push_back(s);
            }
            i += 1;
        }
        // Fin de lecture : laisser le ring se vider puis fermer
        loop {
            let remaining = {
                let r = ring_feed.lock().unwrap();
                r.len()
            };
            if remaining == 0 || stop.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // drop(_stream) ici → coupure audio
        println!("   ✅ Lecture double canaux terminée");
    });

    Ok(())
}

/// Arrête la lecture double canaux en cours (le feeder drop le stream).
pub fn stop_dual() {
    dual_stop_flag().store(true, Ordering::Relaxed);
    println!("   🛑 Lecture double canaux : arrêt demandé");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_conserve_longueur_et_valeurs() {
        let src: Vec<i16> = (0..4410).map(|i| (i % 100) as i16).collect();
        let up = resample_i16(&src, 1, 44100, 88200);
        assert_eq!(up.len(), 8820, "upsampling 2× → 2× de frames");
        let down = resample_i16(&src, 1, 44100, 22050);
        assert_eq!(down.len(), 2205, "downsampling /2 → moitié de frames");
        // Constante → constante (aucune distorsion)
        let cst = vec![1234i16; 1000];
        let cst_up = resample_i16(&cst, 1, 44100, 48000);
        assert_eq!(cst_up.len(), 1088);
        assert!(cst_up.iter().all(|&s| s == 1234));
    }

    /// Frame de départ de la lecture (offset en secondes × fréquence device).
    #[test]
    fn playback_start_frame_offsets() {
        assert_eq!(playback_start_frame(0.0, 44100), 0);
        assert_eq!(playback_start_frame(-3.0, 44100), 0, "négatif → 0");
        assert_eq!(playback_start_frame(1.0, 44100), 44100);
        assert_eq!(playback_start_frame(1.5, 48000), 72000);
        assert_eq!(playback_start_frame(2.0, 22050), 44100);
    }

    /// Boucle (repeat) : la lecture repart au début quand le buffer est fini.
    #[test]
    fn next_frame_loop_repart_a_zero() {
        assert_eq!(next_frame(0, 100, 0, 100, false), Some(0));
        assert_eq!(next_frame(99, 100, 0, 100, false), Some(99));
        assert_eq!(next_frame(100, 100, 0, 100, false), None, "fin sans boucle → stop");
        assert_eq!(next_frame(100, 100, 0, 100, true), Some(0), "fin avec boucle → retour au début");
        assert_eq!(next_frame(101, 100, 0, 100, true), Some(0), "au-delà de la fin → retour au début");
        assert_eq!(next_frame(0, 0, 0, 0, true), None, "buffer vide → rien");
    }

    /// Boucle sur un INTERVALLE (locators [L, R[) : la lecture repart à L
    /// quand elle atteint R, même si le buffer continue après.
    #[test]
    fn next_frame_loop_intervalle_locators() {
        // Intervalle [100, 200[ dans un buffer de 300 frames
        assert_eq!(next_frame(50, 300, 100, 200, true), Some(50), "avant L → joué (passe 0)");
        assert_eq!(next_frame(199, 300, 100, 200, true), Some(199));
        assert_eq!(next_frame(200, 300, 100, 200, true), Some(100), "R atteint → retour à L");
        assert_eq!(next_frame(250, 300, 100, 200, true), Some(100), "au-delà de R → retour à L");
        assert_eq!(next_frame(200, 300, 100, 200, false), None, "sans boucle → fin à R");
        // Intervalle invalide (L ≥ R) → comportement boucle complète
        assert_eq!(next_frame(250, 300, 250, 100, true), Some(0));
    }
}
