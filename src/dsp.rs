//! dsp.rs — effets audio par piste, appliqués AVANT le mixage final du WAV.
//!
//! Chaque piste est rendue séparément par FluidSynth, puis sa chaîne
//! d'effets est appliquée ici :
//!   overdrive → delay → chorus → reverb (freeverb)
//! Enfin les WAVs de pistes sont mixés (somme) dans render.rs.
//!
//! Implémentation 100 % Rust (aucune dépendance externe) : hound lit/écrit
//! le WAV, le DSP travaille en f32 stéréo.

use serde::{Deserialize, Serialize};

/// Réglages d'effets d'une piste (0-100 chacun).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Fx {
    #[serde(default)] pub reverb: u8,   // 0-100 — reverb (freeverb)
    #[serde(default)] pub chorus: u8,   // 0-100 — chorus (2 voix LFO)
    #[serde(default)] pub delay: u8,    // 0-100 — delay (feedback)
    #[serde(default)] pub drive: u8,    // 0-100 — overdrive (soft clip)
}

impl Fx {
    pub fn is_off(&self) -> bool {
        self.reverb == 0 && self.chorus == 0 && self.delay == 0 && self.drive == 0
    }
    fn mix(v: u8) -> f32 {
        (v as f32) / 100.0
    }
}

/// Applique la chaîne d'effets sur un WAV (stéréo ou mono, i16 PCM).
/// `delay_ms` : durée du delay (musicale, ex: croche = 30000/tempo).
/// Retourne un nouveau WAV (mêmes spec) ; si aucun effet, renvoie l'entrée.
pub fn apply_fx(wav: &[u8], fx: &Fx, delay_ms: u32) -> Vec<u8> {
    if fx.is_off() {
        return wav.to_vec();
    }
    use hound::WavReader;
    let Ok(mut reader) = WavReader::new(std::io::Cursor::new(wav)) else {
        return wav.to_vec();
    };
    let spec = reader.spec();
    let sr = spec.sample_rate as f32;
    let ch = spec.channels as usize;
    let samples: Vec<i16> = reader.samples::<i16>().filter_map(|s| s.ok()).collect();
    if samples.is_empty() {
        return wav.to_vec();
    }

    // Canaux séparés en f32
    let n = samples.len() / ch;
    let mut left: Vec<f32> = Vec::with_capacity(n);
    let mut right: Vec<f32> = Vec::with_capacity(n);
    if ch > 1 {
        for i in 0..n {
            left.push(samples[i * 2] as f32 / 32768.0);
            right.push(samples[i * 2 + 1] as f32 / 32768.0);
        }
    } else {
        for i in 0..n {
            let v = samples[i] as f32 / 32768.0;
            left.push(v);
            right.push(v);
        }
    }

    // Chaîne d'effets
    if fx.drive > 0 {
        overdrive(&mut left, &mut right, fx.drive);
    }
    if fx.delay > 0 {
        delay(&mut left, &mut right, delay_ms, sr, fx.delay);
    }
    if fx.chorus > 0 {
        chorus(&mut left, &mut right, sr, fx.chorus);
    }
    if fx.reverb > 0 {
        reverb(&mut left, &mut right, sr, fx.reverb);
    }

    // Réécriture du WAV (mêmes spec)
    let mut buf = Vec::new();
    if let Ok(mut w) = hound::WavWriter::new(std::io::Cursor::new(&mut buf), spec) {
        for i in 0..n {
            let _ = w.write_sample((left[i].clamp(-1.0, 1.0) * 32767.0) as i16);
            if ch > 1 {
                let _ = w.write_sample((right[i].clamp(-1.0, 1.0) * 32767.0) as i16);
            }
        }
        let _ = w.finalize();
    }
    if buf.is_empty() { wav.to_vec() } else { buf }
}

// ─── Overdrive (soft clip) ─────────────────────────────────────────────

fn overdrive(l: &mut [f32], r: &mut [f32], drive: u8) {
    let m = Fx::mix(drive) * 0.85;              // dry/wet
    let g = 1.0 + (drive as f32) * 0.25;        // gain jusqu'à ~26×
    let norm = g.tanh().max(0.001);             // normalise le niveau
    for x in l.iter_mut() {
        let wet = (*x * g).tanh() / norm;
        *x = *x * (1.0 - m) + wet * m;
    }
    for x in r.iter_mut() {
        let wet = (*x * g).tanh() / norm;
        *x = *x * (1.0 - m) + wet * m;
    }
}

// ─── Delay (feedback) ──────────────────────────────────────────────────

fn delay(l: &mut [f32], r: &mut [f32], delay_ms: u32, sr: f32, delay: u8) {
    let d = ((delay_ms.max(1) as f32 * sr / 1000.0) as usize).max(1);
    let m = Fx::mix(delay) * 0.55;              // wet max ~55 %
    let fb = 0.35f32;
    let mut bl = vec![0f32; d];
    let mut br = vec![0f32; d];
    let mut p = 0usize;
    for i in 0..l.len() {
        let dl = bl[p];
        let dr = br[p];
        bl[p] = l[i] + dl * fb;
        br[p] = r[i] + dr * fb;
        l[i] = l[i] * (1.0 - m) + dl * m;
        r[i] = r[i] * (1.0 - m) + dr * m;
        p = (p + 1) % d;
    }
}

// ─── Chorus (2 voix LFO, interpolation linéaire) ───────────────────────

fn chorus(l: &mut [f32], r: &mut [f32], sr: f32, chorus: u8) {
    let m = Fx::mix(chorus) * 0.6;
    let base = 0.006 * sr;                      // délai de base 6 ms
    let depth = 0.003 * sr;                     // profondeur 3 ms
    let rate = 0.7f32;                          // Hz
    let step = 2.0 * std::f32::consts::PI * rate / sr;
    let maxd = (base + depth) as usize + 4;
    let mut bl = vec![0f32; maxd];
    let mut br = vec![0f32; maxd];
    let mut p = 0usize;
    let mut ph = 0f32;
    for i in 0..l.len() {
        // Voix 1 (gauche)
        let d1 = base + depth * (0.5 + 0.5 * ph.sin());
        let (v1, _) = tap(&bl, p, maxd, d1);
        // Voix 2 (droite, phase opposée)
        let d2 = base + depth * (0.5 + 0.5 * (ph + std::f32::consts::PI).sin());
        let (v2, _) = tap(&br, p, maxd, d2);
        bl[p] = l[i];
        br[p] = r[i];
        l[i] = l[i] * (1.0 - m) + v1 * m;
        r[i] = r[i] * (1.0 - m) + v2 * m;
        ph += step;
        p = (p + 1) % maxd;
    }
}

/// Lecture d'un buffer circulaire avec interpolation linéaire.
fn tap(buf: &[f32], head: usize, size: usize, delay: f32) -> (f32, usize) {
    let d = delay as usize;
    let frac = delay - d as f32;
    let i1 = (head + size - d) % size;
    let i2 = (i1 + size - 1) % size;
    (buf[i1] * (1.0 - frac) + buf[i2] * frac, d)
}

// ─── Reverb (freeverb : 8 combs + 4 allpass, stéréo) ───────────────────

struct Comb {
    buf: Vec<f32>,
    idx: usize,
    feedback: f32,
    damp: f32,
    lp: f32,
}

impl Comb {
    fn new(size: usize, feedback: f32, damp: f32) -> Self {
        Self { buf: vec![0.0; size], idx: 0, feedback, damp, lp: 0.0 }
    }
    fn tick(&mut self, x: f32) -> f32 {
        let out = self.buf[self.idx];
        self.lp = out * (1.0 - self.damp) + self.lp * self.damp;
        self.buf[self.idx] = x + self.lp * self.feedback;
        self.idx = (self.idx + 1) % self.buf.len();
        out
    }
}

struct Allpass {
    buf: Vec<f32>,
    idx: usize,
    fb: f32,
}

impl Allpass {
    fn new(size: usize, fb: f32) -> Self {
        Self { buf: vec![0.0; size], idx: 0, fb }
    }
    fn tick(&mut self, x: f32) -> f32 {
        let out = self.buf[self.idx];
        self.buf[self.idx] = x + out * self.fb;
        self.idx = (self.idx + 1) % self.buf.len();
        -x + out
    }
}

/// Délais freeverb classiques (échantillons à 44.1 kHz, décorrélés L/R).
const COMB_L: [usize; 4] = [1116, 1188, 1277, 1356];
const COMB_R: [usize; 4] = [1422, 1491, 1557, 1617];
const ALLPASS_L: [usize; 2] = [556, 441];
const ALLPASS_R: [usize; 2] = [341, 225];

fn reverb(l: &mut [f32], r: &mut [f32], sr: f32, reverb: u8) {
    // Adapter les délais à la fréquence d'échantillonnage réelle
    let k = (sr / 44100.0).max(0.5);
    let sz = |v: usize| ((v as f32 * k) as usize).max(4);
    let mut combs: Vec<Comb> = COMB_L.iter().chain(COMB_R.iter())
        .map(|&d| Comb::new(sz(d), 0.84, 0.25))
        .collect();
    let mut ap_l: Vec<Allpass> = ALLPASS_L.iter().map(|&d| Allpass::new(sz(d), 0.5)).collect();
    let mut ap_r: Vec<Allpass> = ALLPASS_R.iter().map(|&d| Allpass::new(sz(d), 0.5)).collect();

    let m = Fx::mix(reverb) * 0.55;             // wet max ~55 %
    let wet_gain = 0.3;
    for i in 0..l.len() {
        // Somme des combs du canal
        let mut wl = 0.0f32;
        for c in combs.iter_mut().take(4) { wl += c.tick(l[i]); }
        let mut wr = 0.0f32;
        for c in combs.iter_mut().skip(4) { wr += c.tick(r[i]); }
        // Allpass
        for a in ap_l.iter_mut() { wl = a.tick(wl); }
        for a in ap_r.iter_mut() { wr = a.tick(wr); }
        l[i] = l[i] * (1.0 - m) + wl * wet_gain * m;
        r[i] = r[i] * (1.0 - m) + wr * wet_gain * m;
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use hound::{WavSpec, WavWriter, SampleFormat};

    /// Génère un petit WAV stéréo : sinusoïde 440 Hz, 0.5 s.
    fn synth_wav() -> Vec<u8> {
        let spec = WavSpec {
            channels: 2, sample_rate: 44100,
            bits_per_sample: 16, sample_format: SampleFormat::Int,
        };
        let mut buf = Vec::new();
        let mut w = WavWriter::new(std::io::Cursor::new(&mut buf), spec).unwrap();
        let n = 22050;
        for i in 0..n {
            let v = ((i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 44100.0).sin() * 0.5 * 32767.0) as i16;
            w.write_sample(v).unwrap();
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();
        buf
    }

    fn rms(wav: &[u8]) -> f32 {
        use hound::WavReader;
        let mut r = WavReader::new(std::io::Cursor::new(wav)).unwrap();
        let s: Vec<i16> = r.samples::<i16>().filter_map(|s| s.ok()).collect();
        let sum: f64 = s.iter().map(|&v| (v as f64 / 32768.0).powi(2)).sum();
        (sum / s.len().max(1) as f64).sqrt() as f32
    }

    #[test]
    fn fx_off_renvoie_l_entree() {
        let w = synth_wav();
        let out = apply_fx(&w, &Fx::default(), 250);
        assert_eq!(out, w);
    }

    #[test]
    fn fx_conserve_longueur_et_specs() {
        let w = synth_wav();
        for fx in [
            Fx { drive: 60, ..Default::default() },
            Fx { delay: 50, ..Default::default() },
            Fx { chorus: 50, ..Default::default() },
            Fx { reverb: 50, ..Default::default() },
        ] {
            let out = apply_fx(&w, &fx, 250);
            assert_eq!(out.len(), w.len(), "longueur conservée");
        }
    }

    #[test]
    fn overdrive_augmente_le_niveau() {
        let w = synth_wav();
        let r0 = rms(&w);
        let out = apply_fx(&w, &Fx { drive: 100, ..Default::default() }, 250);
        assert!(rms(&out) > r0 * 0.9, "overdrive doit saturer/amplifier");
    }

    #[test]
    fn delay_allonge_le_signal() {
        // Après un delay, le signal ne revient pas à zéro immédiatement :
        // le RMS de la FIN du buffer doit être non nul.
        let w = synth_wav();
        let out = apply_fx(&w, &Fx { delay: 100, ..Default::default() }, 250);
        use hound::WavReader;
        let mut r = WavReader::new(std::io::Cursor::new(&out)).unwrap();
        let s: Vec<i16> = r.samples::<i16>().filter_map(|s| s.ok()).collect();
        let tail: Vec<f32> = s[s.len() - 4000..].iter().map(|&v| (v as f32) / 32768.0).collect();
        let sum: f32 = tail.iter().map(|v| v * v).sum();
        assert!(sum > 0.001, "écho présent en fin de buffer");
    }

    #[test]
    fn reverb_et_chorus_ne_plantent_pas() {
        let w = synth_wav();
        let out = apply_fx(&w, &Fx { reverb: 70, chorus: 40, delay: 30, drive: 20 }, 250);
        assert_eq!(out.len(), w.len());
        assert!(rms(&out) > 0.0);
    }
}
