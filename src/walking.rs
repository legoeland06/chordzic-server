/// Walking Bass — génération de lignes de basse walking.
///
/// Tessiture : MIN_NOTE (22) à MAX_NOTE (42) = [C1, E2].
/// Les chord tones sont ramenés dans cet intervalle par octave.
pub const MIN_NOTE: u8 = 22;
pub const MAX_NOTE: u8 = 42;

/// Maintient une note dans la tessiture basse.
pub fn bass_clamp(n: u8) -> u8 {
    if n < MIN_NOTE {
        n + 12
    } else if n > MAX_NOTE {
        n - 12
    } else {
        n
    }
}

/// Détermine si un accord est mineur (tierce mineure entre la fondamentale
/// et la 3ème note de l'accord, sans tierce majeure).
///
/// `midi_notes` doit contenir au moins [bass_root, chord_root, ...].
/// On analyse `midi_notes[1..]` (les notes de l'accord, pas la basse).
pub fn is_minor(midi_notes: &[u8]) -> bool {
    if midi_notes.len() < 2 {
        return false;
    }
    let root = midi_notes[0]; // fondamentale de l'accord (note 1 du tableau)
    let mut has_minor = false;
    let mut has_major = false;
    for &n in midi_notes {
        let interval = if n >= root {
            n - root
        } else {
            n + 12 - root
        };
        match interval {
            3 => has_minor = true,
            4 => has_major = true,
            _ => {}
        }
    }
    // Mineur si on a une tierce mineure et pas de tierce majeure
    has_minor && !has_major
}

/// Génère 4 notes de walking bass pour une mesure (4 temps).
///
/// * `chord_notes` — [bass_root, chord_tone1, chord_tone2, ...] en MIDI absolu
/// * `next_root` — fondamentale du prochain accord en MIDI absolu
/// * `seed` — germe aléatoire pour la variation
/// * `minor` — si vrai, l'accord est mineur (influence le temps 2)
pub fn generate_walking_bass(
    chord_notes: &[u8],
    next_root: u8,
    seed: u64,
    minor: bool,
) -> [u8; 4] {
    let root = chord_notes[0];
    // Ramener les chord tones dans l'octave basse
    let chord_tones: Vec<u8> = if chord_notes.len() > 1 {
        chord_notes[1..]
            .iter()
            .map(|&n| bass_clamp(n))
            .collect()
    } else {
        vec![root.saturating_sub(5)] // quinte par défaut
    };
    // Enlever les doublons
    let mut tones: Vec<u8> = chord_tones.clone();
    tones.sort();
    tones.dedup();

    // Temps 1 : fondamentale (ancrage)
    let b1 = root;

    // Temps 2 : si mineur, 50% ton au-dessus, 50% chord tone aléatoire
    let b2 = if minor {
        match seed % 100 {
            0..=24 => root + 2,
            25..=49 => root.wrapping_sub(10),
            _ => {
                let idx2 = (seed as usize) % tones.len();
                tones[idx2]
            }
        }
    } else {
        let idx2 = (seed as usize) % tones.len();
        tones[idx2]
    };

    // Temps 3 : chord tone différent du temps 2
    let filtered: Vec<u8> = tones
        .iter()
        .filter(|&&n| n != b2)
        .copied()
        .collect();
    let b3 = if filtered.is_empty() {
        b2 + 7
    } else {
        filtered[(seed.wrapping_add(7) as usize) % filtered.len()]
    };

    // Temps 4 : note d'approche vers next_root
    let b4 = match (seed % 100) as u8 {
        0..=49 => {
            // Approche chromatique (50%)
            let dir = if seed % 2 == 0 { 1i8 } else { -1i8 };
            let mut app = (next_root as i16 + dir as i16) as u8;
            if app < MIN_NOTE || app > MAX_NOTE {
                app = (next_root as i16 - dir as i16) as u8;
            }
            if app < MIN_NOTE {
                app = next_root + 12;
            }
            if app > MAX_NOTE {
                app = next_root - 12;
            }
            app
        }
        50..=67 => {
            // Approche dominante (35%)
            let mut app = next_root + 7;
            if app > MAX_NOTE {
                app -= 12;
            }
            app
        }
        68..=85 => {
            // Approche sous-dominante (15%)
            let mut app = next_root.wrapping_sub(5);
            if app < MIN_NOTE {
                app += 12;
            }
            app
        }
        _ => {
            // Approche diatonique (15%) : chord tone le plus proche
            tones
                .iter()
                .min_by_key(|&&t| {
                    let diff = if t > next_root {
                        t - next_root
                    } else {
                        next_root - t
                    };
                    diff
                })
                .copied()
                .unwrap_or(next_root)
        }
    };

    [b1, b2, b3, b4]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bass_clamp() {
        assert_eq!(bass_clamp(36), 36); // C2, déjà dans la tessiture
        assert_eq!(bass_clamp(48), 36); // C3 → C2 (descend d'une octave)
        assert_eq!(bass_clamp(12), 24); // C0 → C1 (monte d'une octave)
        assert_eq!(bass_clamp(22), 22); // limite basse
        assert_eq!(bass_clamp(42), 42); // limite haute
        assert_eq!(bass_clamp(21), 33); // en dessous → +12
        assert_eq!(bass_clamp(43), 31); // au dessus → -12
    }

    #[test]
    fn test_is_minor_major() {
        // Cmaj: C E G → C=48, E=52, G=55
        assert!(!is_minor(&[48, 52, 55]));
        // Cmin: C Eb G → C=48, Eb=51, G=55
        assert!(is_minor(&[48, 51, 55]));
        // Accord vide → pas mineur
        assert!(!is_minor(&[]));
        // Une seule note → pas mineur
        assert!(!is_minor(&[48]));
    }

    #[test]
    fn test_generate_walking_bass_length() {
        // Cmaj: C=48, E=52, G=55
        let notes = generate_walking_bass(&[48, 52, 55], 53, 0, false);
        assert_eq!(notes.len(), 4);
    }

    #[test]
    fn test_generate_walking_bass_first_note_is_root() {
        let notes = generate_walking_bass(&[48, 52, 55], 53, 0, false);
        assert_eq!(notes[0], 48, "Le temps 1 doit être la fondamentale");
    }

    #[test]
    fn test_generate_walking_bass_minor_second_note() {
        // La 2e note d'un mineur a 50% de chance d'être root+2
        let notes = generate_walking_bass(&[48, 51, 55], 53, 0, true);
        // seed=0 → 0..=24 → root+2
        assert_eq!(notes[1], 50); // 48+2
    }

    #[test]
    fn test_generate_walking_bass_approche_chromatique() {
        // Cmaj → Fmaj (next_root=53). seed=0 → approche chromatique
        // 53+1=54 > MAX_NOTE → 53-1=52 > MAX_NOTE → 53-12=41
        let notes = generate_walking_bass(&[48, 52, 55], 53, 0, false);
        assert_eq!(notes[3], 41, "l'approche chromatique doit rester dans la tessiture");
    }

    #[test]
    fn test_bass_clamp_stability() {
        // Vérifier que clamp ne change pas une note déjà dans la tessiture
        for n in MIN_NOTE..=MAX_NOTE {
            assert_eq!(bass_clamp(n), n, "bass_clamp({n}) devrait être {n}");
        }
    }
}
