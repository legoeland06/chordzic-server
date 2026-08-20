//! mpe.rs — Émulation de contrôleur MPE (MIDI Polyphonic Expression).
//!
//! La modal frontend « 🎛 MPE » simule un contrôleur d'expression (type ROLI
//! Seaboard / LinnStrument / Osmose) : l'utilisateur glisse / presse pour
//! moduler le son EN DIRECT pendant qu'il joue sur le Roland (Local Control
//! OFF → le serveur relaie les notes) ou pendant un enregistrement.
//!
//! Niveau N1 (temps réel, global par canal) :
//!   - Pitch Bend 14-bit (glissé horizontal) — range réglable via RPN 0 ;
//!   - Channel Pressure (pression) ;
//!   - CC74 timbre/brightness (glissé vertical) ;
//!   - LFO optionnel (vibrato auto sur le bend).
//!
//! Niveau N2 (enregistrement) : les changements d'expression sont horodatés
//! dans la session Rec (`RecExpr`) pour être réappliqués au rendu WAV.
//!
//! Le module est 100 % fonctions pures (testable sans matériel) — l'état
//! partagé vit dans `LiveInputState` (comme `echo` et `rec`).

/// Centre du pitch bend 14-bit (0-16383) : ni aigu ni grave.
pub const BEND_CENTER: u16 = 8192;
/// Valeur neutre du timbre CC74 (GM2 brightness).
pub const TIMBRE_CENTER: u8 = 64;

/// Forme d'onde du LFO (vibrato auto).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LfoShape {
    Sin,
    Triangle,
    Square,
}

impl Default for LfoShape {
    fn default() -> Self {
        LfoShape::Sin
    }
}

/// Configuration du LFO (vibrato auto sur le bend). Désactivé si freq = 0.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Lfo {
    /// Fréquence en Hz (0 = désactivé).
    pub freq: f32,
    /// Profondeur en demi-tons (0-24, arrondie au centième près à l'envoi).
    pub depth_st: f32,
    /// Forme d'onde.
    pub shape: LfoShape,
}

impl Default for Lfo {
    fn default() -> Self {
        Self { freq: 0.0, depth_st: 0.0, shape: LfoShape::Sin }
    }
}

impl Lfo {
    pub fn is_off(&self) -> bool {
        self.freq <= 0.0 || self.depth_st <= 0.0
    }
}

/// État MPE partagé (N1) : les valeurs courantes d'expression + la cible.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MpeState {
    /// Monitoring MPE actif ? (modal ouverte → le serveur relaie les notes
    /// du pianiste sur le canal cible et y injecte les modulations).
    pub enabled: bool,
    /// Pitch bend courant (0-16383, centre 8192).
    pub bend: u16,
    /// Channel pressure courant (0-127).
    pub pressure: u8,
    /// Timbre CC74 courant (0-127).
    pub timbre: u8,
    /// Range de pitch bend en demi-tons (RPN 0), défaut 48 (spec MPE).
    pub pitch_range_st: u8,
    /// LFO (vibrato auto).
    pub lfo: Lfo,
    /// Canal cible explicite (None = auto : canal d'écho ✨ si actif, sinon 1).
    pub channel: Option<u8>,
}

impl Default for MpeState {
    fn default() -> Self {
        Self {
            enabled: false,
            bend: BEND_CENTER,
            pressure: 0,
            timbre: TIMBRE_CENTER,
            pitch_range_st: 48,
            lfo: Lfo::default(),
            channel: None,
        }
    }
}

impl MpeState {
    /// Vrai si l'état est neutre (rien à envoyer de particulier).
    pub fn is_neutral(&self) -> bool {
        self.bend == BEND_CENTER && self.pressure == 0 && self.timbre == TIMBRE_CENTER
    }

    /// Remet l'expression à zéro (bend centre, AT 0, CC74 neutre).
    pub fn reset(&mut self) {
        self.bend = BEND_CENTER;
        self.pressure = 0;
        self.timbre = TIMBRE_CENTER;
    }
}

// ── Builders de messages MIDI (purs) ──────────────────────────────────

/// Pitch bend 14-bit sur un canal : `[0xE0|ch, lsb7, msb7]`.
pub fn pitch_bend_message(channel: u8, v14: u16) -> Vec<u8> {
    let v = v14.clamp(0, 16383);
    vec![0xE0 | (channel & 0x0F), (v & 0x7F) as u8, ((v >> 7) & 0x7F) as u8]
}

/// Channel pressure (aftertouch) sur un canal : `[0xD0|ch, val]`.
pub fn channel_pressure_message(channel: u8, value: u8) -> Vec<u8> {
    vec![0xD0 | (channel & 0x0F), value.min(127)]
}

/// CC générique sur un canal : `[0xB0|ch, ctl, val]`.
pub fn cc_message(channel: u8, ctl: u8, value: u8) -> Vec<u8> {
    vec![0xB0 | (channel & 0x0F), ctl, value.min(127)]
}

/// CC74 (timbre/brightness) sur un canal.
pub fn timbre_message(channel: u8, value: u8) -> Vec<u8> {
    cc_message(channel, 74, value)
}

/// Séquence RPN 0 (pitch bend range) : règle le range en demi-tons sur le
/// canal. Séquence RPN classique :
///   B0 65 00 (RPN MSB = 0) · B0 64 00 (RPN LSB = 0) · B0 06 NN (data MSB)
///   · B0 26 00 (data LSB) · B0 65 7F · B0 64 7F (fin de RPN)
pub fn rpn_pitch_range_messages(channel: u8, semitones: u8) -> Vec<Vec<u8>> {
    let st = semitones.min(127);
    vec![
        vec![0xB0 | (channel & 0x0F), 101, 0],
        vec![0xB0 | (channel & 0x0F), 100, 0],
        vec![0xB0 | (channel & 0x0F), 6, st],
        vec![0xB0 | (channel & 0x0F), 38, 0],
        vec![0xB0 | (channel & 0x0F), 101, 127],
        vec![0xB0 | (channel & 0x0F), 100, 127],
    ]
}

/// Messages d'expression complets pour un canal : RPN range (optionnel, si
/// différent de la valeur actuelle du récepteur — géré par l'appelant via
/// `range_dirty`), bend, pressure, timbre.
pub fn expression_messages(
    channel: u8,
    bend: u16,
    pressure: u8,
    timbre: u8,
    with_range: Option<u8>,
) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if let Some(st) = with_range {
        out.extend(rpn_pitch_range_messages(channel, st));
    }
    out.push(pitch_bend_message(channel, bend));
    out.push(channel_pressure_message(channel, pressure));
    out.push(timbre_message(channel, timbre));
    out
}

// ── LFO (vibrato auto) ────────────────────────────────────────────────

/// Offset de bend généré par le LFO à l'instant `t_ms` (en µs de bend).
/// `depth_st` est converti en valeur 14-bit : ±range complet = ±depth_st
/// demi-tons, soit un offset de ±8192 × depth_st / pitch_range_st.
pub fn lfo_bend_offset(t_ms: u64, lfo: &Lfo, pitch_range_st: u8) -> i32 {
    if lfo.is_off() {
        return 0;
    }
    let t = t_ms as f64 / 1000.0;
    let phase = 2.0 * std::f64::consts::PI * lfo.freq as f64 * t;
    let norm = match lfo.shape {
        LfoShape::Sin => phase.sin(),
        LfoShape::Triangle => (phase % (2.0 * std::f64::consts::PI)) / std::f64::consts::PI,
        LfoShape::Square => {
            if (phase % (2.0 * std::f64::consts::PI)) < std::f64::consts::PI {
                1.0
            } else {
                -1.0
            }
        }
    };
    // Triangle : remapper [0, 2π[ → [-1, 1] en dents de scie repliée
    let norm = if lfo.shape == LfoShape::Triangle {
        let p = phase % (2.0 * std::f64::consts::PI);
        if p < std::f64::consts::PI {
            -1.0 + 2.0 * p / std::f64::consts::PI
        } else {
            3.0 - 2.0 * p / std::f64::consts::PI
        }
    } else {
        norm
    };
    let range = if pitch_range_st == 0 { 48.0 } else { pitch_range_st as f64 };
    let max_offset = 8192.0 * (lfo.depth_st as f64) / range;
    (norm * max_offset).round() as i32
}

/// Bend effectif (LFO inclus) : centre + LFO + geste manuel.
pub fn effective_bend(state: &MpeState, t_ms: u64) -> u16 {
    let offset = lfo_bend_offset(t_ms, &state.lfo, state.pitch_range_st);
    let raw = state.bend as i32 + offset;
    raw.clamp(0, 16383) as u16
}

// ── Enregistrement des gestes (N2) ────────────────────────────────────

/// Événement d'expression horodaté pendant une session Rec.
/// Chaque champ Option ne contient que les valeurs CHANGÉES par le geste.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecExpr {
    /// ms depuis le début de la session (même base que RecNote).
    pub t_ms: u64,
    /// Pitch bend (centre 8192).
    pub bend: Option<u16>,
    /// Channel pressure (0-127).
    pub pressure: Option<u8>,
    /// Timbre CC74 (0-127).
    pub timbre: Option<u8>,
    /// Range de bend posé (RPN 0) à cet instant — None si inchangé.
    pub pitch_range_st: Option<u8>,
    /// LFO activé à cet instant (freq Hz, depth demi-tons) — None si inchangé.
    pub lfo: Option<(f32, f32)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_bend_14_bit_encodage() {
        // 8192 = centre → lsb 0, msb 64
        assert_eq!(pitch_bend_message(1, 8192), vec![0xE1, 0x00, 64]);
        // 0 → lsb 0, msb 0
        assert_eq!(pitch_bend_message(0, 0), vec![0xE0, 0x00, 0]);
        // 16383 → lsb 127, msb 127
        assert_eq!(pitch_bend_message(3, 16383), vec![0xE3, 127, 127]);
        // clamp : 20000 → 16383
        assert_eq!(pitch_bend_message(1, 20000), vec![0xE1, 127, 127]);
    }

    #[test]
    fn channel_pressure_et_cc74() {
        assert_eq!(channel_pressure_message(5, 100), vec![0xD5, 100]);
        assert_eq!(channel_pressure_message(5, 200), vec![0xD5, 127]); // clamp
        assert_eq!(timbre_message(2, 64), vec![0xB2, 74, 64]);
        assert_eq!(timbre_message(2, 200), vec![0xB2, 74, 127]); // clamp
    }

    #[test]
    fn rpn_pitch_range_sequence_complete() {
        let msgs = rpn_pitch_range_messages(2, 48);
        assert_eq!(
            msgs,
            vec![
                vec![0xB2, 101, 0],
                vec![0xB2, 100, 0],
                vec![0xB2, 6, 48],
                vec![0xB2, 38, 0],
                vec![0xB2, 101, 127],
                vec![0xB2, 100, 127],
            ]
        );
    }

    #[test]
    fn expression_messages_avec_et_sans_range() {
        let with = expression_messages(1, 9000, 50, 80, Some(48));
        assert_eq!(with.len(), 3 + 6); // RPN (6) + bend + AT + CC74
        assert_eq!(with[6], vec![0xE1, (9000 & 0x7F) as u8, ((9000 >> 7) & 0x7F) as u8]);
        assert_eq!(with[7], vec![0xD1, 50]);
        assert_eq!(with[8], vec![0xB1, 74, 80]);

        let without = expression_messages(1, 8192, 0, 64, None);
        assert_eq!(without.len(), 3);
    }

    #[test]
    fn lfo_sinus_oscille_autour_du_centre() {
        let lfo = Lfo { freq: 1.0, depth_st: 12.0, shape: LfoShape::Sin };
        // t=0 → sin(0) = 0
        assert_eq!(lfo_bend_offset(0, &lfo, 48), 0);
        // t=250 ms → sin(π/2) = 1 → +8192 × 12/48 = +2048
        assert_eq!(lfo_bend_offset(250, &lfo, 48), 2048);
        // t=750 ms → sin(3π/2) = -1 → -2048
        assert_eq!(lfo_bend_offset(750, &lfo, 48), -2048);
        // t=500 ms → sin(π) ≈ 0
        assert_eq!(lfo_bend_offset(500, &lfo, 48), 0);
    }

    #[test]
    fn lfo_off_ou_profondeur_nulle_renvoie_zero() {
        let off = Lfo::default();
        assert_eq!(lfo_bend_offset(1234, &off, 48), 0);
        let depth_zero = Lfo { freq: 5.0, depth_st: 0.0, shape: LfoShape::Sin };
        assert_eq!(lfo_bend_offset(1234, &depth_zero, 48), 0);
    }

    #[test]
    fn lfo_carre_bascule_entre_extremes() {
        let lfo = Lfo { freq: 1.0, depth_st: 24.0, shape: LfoShape::Square };
        // t=0 → +1 → +8192 × 24/48 = +4096
        assert_eq!(lfo_bend_offset(0, &lfo, 48), 4096);
        // t=600 ms → π < phase < 2π → -1 → -4096
        assert_eq!(lfo_bend_offset(600, &lfo, 48), -4096);
    }

    #[test]
    fn lfo_triangle_borne_aux_extremes() {
        let lfo = Lfo { freq: 1.0, depth_st: 48.0, shape: LfoShape::Triangle };
        // Triangle linéaire : -1 à p=0, 0 à p=π/2, +1 à p=π, 0 à p=3π/2
        assert_eq!(lfo_bend_offset(0, &lfo, 48), -8192);
        assert_eq!(lfo_bend_offset(250, &lfo, 48), 0);
        assert_eq!(lfo_bend_offset(500, &lfo, 48), 8192);
        assert_eq!(lfo_bend_offset(750, &lfo, 48), 0);
    }

    #[test]
    fn effective_bend_borne_entre_0_et_16383() {
        let mut st = MpeState::default();
        st.bend = 0; // bend au plus bas
        st.lfo = Lfo { freq: 1.0, depth_st: 48.0, shape: LfoShape::Square };
        // t=0 → +8192 → clamp à 16383 (0 + 8192 = 8192, pas de dépassement)
        assert_eq!(effective_bend(&st, 0), 8192);
        st.bend = 10000;
        st.lfo = Lfo { freq: 1.0, depth_st: 24.0, shape: LfoShape::Square };
        // t=600 → -4096 → 10000 - 4096 = 5904
        assert_eq!(effective_bend(&st, 600), 5904);
        // Dépassement haut : bend 16383 + LFO + → clamp 16383
        st.bend = 16383;
        st.lfo = Lfo { freq: 1.0, depth_st: 24.0, shape: LfoShape::Square };
        assert_eq!(effective_bend(&st, 0), 16383);
    }

    #[test]
    fn mpe_state_reset_et_neutre() {
        let mut st = MpeState::default();
        assert!(st.is_neutral());
        st.bend = 9000;
        st.pressure = 100;
        st.timbre = 20;
        assert!(!st.is_neutral());
        st.reset();
        assert!(st.is_neutral());
        assert_eq!(st.bend, BEND_CENTER);
        assert_eq!(st.pressure, 0);
        assert_eq!(st.timbre, TIMBRE_CENTER);
    }
}
