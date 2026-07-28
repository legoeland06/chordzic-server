/// Synthèse audio via FluidSynth (libfluidsynth C).
/// Remplaçant rustysynth : même qualité sonore que FluidSynth standalone.
/// Produit des samples PCM (stéréo float32) dans un buffer circulaire,
/// consommé par le streaming WebSocket.
use std::collections::VecDeque;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

// ─── Types partagés ────────────────────────────────────────────────────
pub type AudioBuffer = Arc<Mutex<VecDeque<f32>>>;

// ─── FFI FluidSynth ────────────────────────────────────────────────────
#[allow(non_camel_case_types, dead_code)]
type fluid_synth_t = std::ffi::c_void;
#[allow(non_camel_case_types, dead_code)]
type fluid_settings_t = std::ffi::c_void;

extern "C" {
    fn new_fluid_settings() -> *mut fluid_settings_t;
    fn delete_fluid_settings(settings: *mut fluid_settings_t);
    fn fluid_settings_setstr(
        settings: *mut fluid_settings_t,
        name: *const std::os::raw::c_char,
        val: *const std::os::raw::c_char,
    ) -> std::os::raw::c_int;
    fn fluid_settings_setnum(
        settings: *mut fluid_settings_t,
        name: *const std::os::raw::c_char,
        val: f64,
    ) -> std::os::raw::c_int;
    fn fluid_settings_setint(
        settings: *mut fluid_settings_t,
        name: *const std::os::raw::c_char,
        val: std::os::raw::c_int,
    ) -> std::os::raw::c_int;

    fn new_fluid_synth(settings: *mut fluid_settings_t) -> *mut fluid_synth_t;
    fn delete_fluid_synth(synth: *mut fluid_synth_t);

    fn fluid_synth_sfload(
        synth: *mut fluid_synth_t,
        filename: *const std::os::raw::c_char,
        reset_presets: std::os::raw::c_int,
    ) -> std::os::raw::c_int;

    fn fluid_synth_noteon(
        synth: *mut fluid_synth_t,
        chan: std::os::raw::c_int,
        key: std::os::raw::c_int,
        vel: std::os::raw::c_int,
    ) -> std::os::raw::c_int;
    fn fluid_synth_noteoff(
        synth: *mut fluid_synth_t,
        chan: std::os::raw::c_int,
        key: std::os::raw::c_int,
    ) -> std::os::raw::c_int;
    fn fluid_synth_cc(
        synth: *mut fluid_synth_t,
        chan: std::os::raw::c_int,
        ctrl: std::os::raw::c_int,
        val: std::os::raw::c_int,
    ) -> std::os::raw::c_int;
    fn fluid_synth_program_change(
        synth: *mut fluid_synth_t,
        chan: std::os::raw::c_int,
        prog: std::os::raw::c_int,
    ) -> std::os::raw::c_int;
    fn fluid_synth_pitch_bend(
        synth: *mut fluid_synth_t,
        chan: std::os::raw::c_int,
        val: std::os::raw::c_int,
    ) -> std::os::raw::c_int;
    fn fluid_synth_all_notes_off(
        synth: *mut fluid_synth_t,
        chan: std::os::raw::c_int,
    ) -> std::os::raw::c_int;

    fn fluid_synth_write_float(
        synth: *mut fluid_synth_t,
        len: std::os::raw::c_int,
        lout: *mut f32,
        loff: std::os::raw::c_int,
        linc: std::os::raw::c_int,
        rout: *mut f32,
        roff: std::os::raw::c_int,
        rinc: std::os::raw::c_int,
    );
}

// ─── Interface MIDI commune ────────────────────────────────────────────
pub trait SynthOut: Send {
    fn note_on(&mut self, ch: u8, note: u8, vel: u8);
    fn note_off(&mut self, ch: u8, note: u8);
    fn program_change(&mut self, ch: u8, prog: u8);
    fn pitch_bend(&mut self, ch: u8, value: u16);
    fn control_change(&mut self, ch: u8, ctl: u8, val: u8);
    fn all_notes_off(&mut self);

    /// Rendre des échantillons audio (no-op pour MIDI out).
    fn render_audio(&mut self, _frames: usize) {}
    /// Vider le buffer audio (no-op pour MIDI out).
    fn reset_buffer(&mut self) {}
    fn on_stop(&mut self) {}
}

// ─── FluidSynthRenderer : synthèse via libfluidsynth ───────────────────
pub struct FluidSynthRenderer {
    synth: *mut fluid_synth_t,
    settings: *mut fluid_settings_t,
    buffer: AudioBuffer,
    sample_rate: u32,
    _running: Arc<AtomicBool>,
}

unsafe impl Send for FluidSynthRenderer {}
unsafe impl Sync for FluidSynthRenderer {}

impl FluidSynthRenderer {
    /// Crée le synthétiseur FluidSynth avec un SoundFont.
    /// Retourne (renderer, audio_buffer).
    pub fn new(sf_path: &str, sample_rate: u32) -> Result<(Self, AudioBuffer), String> {
        let settings = unsafe { new_fluid_settings() };
        if settings.is_null() {
            return Err("Échec new_fluid_settings".into());
        }

        // Configurer le sample rate
        let sr_name = CString::new("synth.sample-rate").unwrap();
        unsafe {
            fluid_settings_setnum(settings, sr_name.as_ptr(), sample_rate as f64);
        }

        // Désactiver la sortie audio (on veut juste le rendu en mémoire)
        let drv_name = CString::new("audio.driver").unwrap();
        let drv_val = CString::new("file").unwrap();
        unsafe {
            fluid_settings_setstr(settings, drv_name.as_ptr(), drv_val.as_ptr());
        }

        let synth = unsafe { new_fluid_synth(settings) };
        if synth.is_null() {
            unsafe { delete_fluid_settings(settings) }
            return Err("Échec new_fluid_synth".into());
        }

        // Charger le SoundFont
        let sf_cstr = CString::new(sf_path).map_err(|_| "Chemin SoundFont invalide".to_string())?;
        let sf_id = unsafe { fluid_synth_sfload(synth, sf_cstr.as_ptr(), 1) };
        if sf_id < 0 {
            unsafe {
                delete_fluid_synth(synth);
                delete_fluid_settings(settings);
            }
            return Err(format!("Impossible de charger le SoundFont: {}", sf_path));
        }

        let buffer = Arc::new(Mutex::new(VecDeque::with_capacity(
            (sample_rate as usize) * 2,
        )));
        let running = Arc::new(AtomicBool::new(false));

        println!(
            "   ✅ FluidSynth embedded : {} (sfid={})",
            sf_path, sf_id
        );

        Ok((
            Self {
                synth,
                settings,
                buffer: buffer.clone(),
                sample_rate,
                _running: running,
            },
            buffer,
        ))
    }

    /// Rend `frames` échantillons stéréo et les pousse dans le buffer.
    pub fn render_frames(&mut self, frames: usize) {
        if frames == 0 || self.synth.is_null() {
            return;
        }
        let mut left = vec![0.0f32; frames];
        let mut right = vec![0.0f32; frames];

        unsafe {
            fluid_synth_write_float(
                self.synth,
                frames as std::os::raw::c_int,
                left.as_mut_ptr(),
                0,
                1,
                right.as_mut_ptr(),
                0,
                1,
            );
        }

        let mut buf = self.buffer.lock().unwrap();
        for i in 0..frames {
            buf.push_back(left[i]);
            buf.push_back(right[i]);
        }
    }

    pub fn clear_buffer(&mut self) {
        self.buffer.lock().unwrap().clear();
    }

    pub fn reset(&mut self) {
        if !self.synth.is_null() {
            for ch in 0..16 {
                unsafe {
                    fluid_synth_all_notes_off(self.synth, ch);
                }
            }
        }
        self.clear_buffer();
    }
}

impl Drop for FluidSynthRenderer {
    fn drop(&mut self) {
        if !self.synth.is_null() {
            unsafe {
                delete_fluid_synth(self.synth);
            }
        }
        if !self.settings.is_null() {
            unsafe {
                delete_fluid_settings(self.settings);
            }
        }
    }
}

impl SynthOut for FluidSynthRenderer {
    fn note_on(&mut self, ch: u8, note: u8, vel: u8) {
        if !self.synth.is_null() {
            unsafe {
                fluid_synth_noteon(self.synth, ch as std::os::raw::c_int, note as std::os::raw::c_int, vel as std::os::raw::c_int);
            }
        }
    }

    fn note_off(&mut self, ch: u8, note: u8) {
        if !self.synth.is_null() {
            unsafe {
                fluid_synth_noteoff(self.synth, ch as std::os::raw::c_int, note as std::os::raw::c_int);
            }
        }
    }

    fn program_change(&mut self, ch: u8, prog: u8) {
        if !self.synth.is_null() {
            unsafe {
                fluid_synth_program_change(self.synth, ch as std::os::raw::c_int, prog as std::os::raw::c_int);
            }
        }
    }

    fn pitch_bend(&mut self, ch: u8, value: u16) {
        if !self.synth.is_null() {
            unsafe {
                fluid_synth_pitch_bend(self.synth, ch as std::os::raw::c_int, value as std::os::raw::c_int);
            }
        }
    }

    fn control_change(&mut self, ch: u8, ctl: u8, val: u8) {
        if !self.synth.is_null() {
            unsafe {
                fluid_synth_cc(self.synth, ch as std::os::raw::c_int, ctl as std::os::raw::c_int, val as std::os::raw::c_int);
            }
        }
    }

    fn all_notes_off(&mut self) {
        if !self.synth.is_null() {
            for ch in 0..16 {
                unsafe {
                    fluid_synth_all_notes_off(self.synth, ch);
                }
            }
        }
    }

    fn render_audio(&mut self, frames: usize) {
        self.render_frames(frames);
    }

    fn reset_buffer(&mut self) {
        self.clear_buffer();
    }

    fn on_stop(&mut self) {
        self.reset();
    }
}

// ─── Implémentation pour midir ─────────────────────────────────────────
impl SynthOut for midir::MidiOutputConnection {
    fn note_on(&mut self, ch: u8, note: u8, vel: u8) {
        let _ = self.send(&[0x90 | ch, note, vel]);
    }
    fn note_off(&mut self, ch: u8, note: u8) {
        let _ = self.send(&[0x80 | ch, note, 64]);
    }
    fn program_change(&mut self, ch: u8, prog: u8) {
        let _ = self.send(&[0xC0 | ch, prog]);
    }
    fn pitch_bend(&mut self, ch: u8, value: u16) {
        let lsb = (value & 127) as u8;
        let msb = ((value >> 7) & 127) as u8;
        let _ = self.send(&[0xE0 | ch, lsb, msb]);
    }
    fn control_change(&mut self, ch: u8, ctl: u8, val: u8) {
        let _ = self.send(&[0xB0 | ch, ctl, val]);
    }
    fn all_notes_off(&mut self) {
        for ch in 0..16 {
            let _ = self.send(&[0xB0 | ch, 123, 0]);
        }
    }
}

// ─── MultiOut : envoie aux deux backends simultanément ──────────────────
pub struct MultiOut<'a> {
    pub fluid: &'a mut FluidSynthRenderer,
    pub midi: &'a mut midir::MidiOutputConnection,
}

impl<'a> SynthOut for MultiOut<'a> {
    fn note_on(&mut self, ch: u8, note: u8, vel: u8) {
        self.fluid.note_on(ch, note, vel);
        let _ = self.midi.send(&[0x90 | ch, note, vel]);
    }
    fn note_off(&mut self, ch: u8, note: u8) {
        self.fluid.note_off(ch, note);
        let _ = self.midi.send(&[0x80 | ch, note, 64]);
    }
    fn program_change(&mut self, ch: u8, prog: u8) {
        self.fluid.program_change(ch, prog);
        let _ = self.midi.send(&[0xC0 | ch, prog]);
    }
    fn pitch_bend(&mut self, ch: u8, value: u16) {
        self.fluid.pitch_bend(ch, value);
        let lsb = (value & 127) as u8;
        let msb = ((value >> 7) & 127) as u8;
        let _ = self.midi.send(&[0xE0 | ch, lsb, msb]);
    }
    fn control_change(&mut self, ch: u8, ctl: u8, val: u8) {
        self.fluid.control_change(ch, ctl, val);
        let _ = self.midi.send(&[0xB0 | ch, ctl, val]);
    }
    fn all_notes_off(&mut self) {
        self.fluid.all_notes_off();
        for ch in 0..16 {
            let _ = self.midi.send(&[0xB0 | ch, 123, 0]);
        }
    }
    fn render_audio(&mut self, frames: usize) {
        self.fluid.render_audio(frames);
    }
    fn reset_buffer(&mut self) {
        self.fluid.reset_buffer();
    }
    fn on_stop(&mut self) {
        self.fluid.on_stop();
    }
}
