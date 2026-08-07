// ─── Piste de clic — MODE RENDU UNIQUEMENT (mode Navig) ────────────────────
//
// Le clic est intégré au WAV rendu (voir render::generate_click_smf + mix) :
// synchronisation échantillon-parfaite par construction. Il n'y a PAS de
// clic live (mode MIDI temps réel) — le clic live était désynchronisé
// (deux horloges audio distinctes) et a été retiré à la demande d'Eric.
//
// Ce module ne garde que la CONFIGURATION du clic (volume, accent, son)
// et l'énumération des sorties audio (cpal) pour le frontend.

use cpal::traits::{DeviceTrait, HostTrait};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

// Sons de clic disponibles (pour le rendu) :
// 0 = Métronome GM (percussion 33/34 — le son classique)
// 1 = Woodblock (GM 115)
// 2 = Agogo (GM 114)
// 3 = Taiko (GM 116)
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
}

impl Default for ClickState {
    fn default() -> Self {
        ClickState {
            volume: AtomicU8::new(80),
            accent: AtomicBool::new(true),
            sound: AtomicU8::new(SOUND_GM_METRONOME),
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

// Petite aide pour le frontend : le nom du son courant (évite de dupliquer
// la liste côté client si besoin). Utilisé par GET /click.
pub fn sound_name(sound: u8) -> &'static str {
    match sound {
        SOUND_WOODBLOCK => "Woodblock",
        SOUND_AGOGO => "Agogo",
        SOUND_TAIKO => "Taiko",
        _ => "Métronome GM",
    }
}

// L'import Ordering est utilisé par les handlers HTTP (via l'état partagé).
pub fn load(state: &ClickState) -> (u8, bool, u8) {
    (
        state.volume.load(Ordering::Relaxed),
        state.accent.load(Ordering::Relaxed),
        state.sound.load(Ordering::Relaxed),
    )
}
