/// Entrée MIDI live — écoute du clavier du pianiste (ex. Roland Digital Piano).
///
/// Ouvre une connexion MIDI ENTRANTE (MidiInput) sur le port du clavier,
/// accumule les notes tenues (tous canaux SAUF le canal drums 9), et expose
/// l'état pour la reconnaissance d'accords côté frontend (route /live-input).
///
/// La reconnaissance elle-même (comparaison avec l'harmonie QUALITY_INTERVALS)
/// se fait côté FRONTEND : le serveur ne fait que relayer les pitchs tenus —
/// pas de duplication de la table d'harmonie, une seule source de vérité.

use midir::{MidiInput, MidiInputConnection};
use std::sync::{Arc, Mutex};

/// Canal drums GM (10 en 1-indexé = 9 en 0-indexé) — jamais pris en compte
/// pour la reconnaissance d'accords (percussions ≠ accords).
pub const DRUM_CHANNEL: u8 = 9;

/// État partagé des notes tenues sur le clavier.
pub struct LiveInputState {
    /// Nom du port écouté (None si aucun clavier détecté).
    pub device: Mutex<Option<String>>,
    /// Pitchs MIDI actuellement tenus (sans doublons, ORDRE D'ARRIVÉE —
    /// l'ordre dans lequel le pianiste plaque les notes est conservé pour
    /// l'insertion fidèle dans le piano roll ; la reconnaissance d'accords
    /// trie elle-même les classes de hauteur).
    pub active: Mutex<Vec<u8>>,
    /// Connexion gardée vivante (si dropped → l'écoute s'arrête).
    _conn: Mutex<Option<MidiInputConnection<()>>>,
}

impl LiveInputState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            device: Mutex::new(None),
            active: Mutex::new(Vec::new()),
            _conn: Mutex::new(None),
        })
    }
}

/// Applique un message MIDI entrant à l'état des notes tenues.
/// Fonction pure (testable sans matériel) :
/// - Note-on (vel > 0) → pitch actif (AJOUTÉ EN FIN, sans tri — l'ordre
///   d'appui du pianiste est conservé)
/// - Note-off OU note-on vel 0 → pitch inactif
/// - Canal drums (9) → ignoré
/// - Autres messages (CC, PC, pitch bend…) → ignorés
pub fn apply_midi_message(active: &mut Vec<u8>, msg: &[u8]) {
    if msg.len() < 2 {
        return;
    }
    let status = msg[0];
    let channel = status & 0x0F;
    if channel == DRUM_CHANNEL {
        return;
    }
    let high = status & 0xF0;
    let pitch = msg[1];
    let is_on = match high {
        0x90 => msg.get(2).copied().unwrap_or(127) > 0, // vel 0 = note-off
        0x80 => false,
        _ => return, // CC, PC, pitch bend… hors sujet
    };
    if is_on {
        if !active.contains(&pitch) {
            active.push(pitch);
        }
    } else {
        active.retain(|&p| p != pitch);
    }
}

/// Ouvre l'écoute sur le clavier : priorité au port contenant « roland »,
/// sinon premier port entrant qui n'est pas « Midi Through »/« System ».
/// Échec silencieux et loggé — la route /live-input renverra device: null.
pub fn start(shared: &Arc<LiveInputState>) {
    let Ok(mi) = MidiInput::new("chords-server-rs-in") else {
        eprintln!("⚠️ Entrée MIDI indisponible — reconnaissance d'accords désactivée");
        return;
    };
    let ports = mi.ports();
    if ports.is_empty() {
        eprintln!("⚠️ Aucun port d'entrée MIDI — reconnaissance d'accords désactivée");
        return;
    }
    let mut chosen: Option<(usize, String)> = None;
    for (i, p) in ports.iter().enumerate() {
        if let Ok(n) = mi.port_name(p) {
            let lower = n.to_lowercase();
            if lower.contains("roland") {
                chosen = Some((i, n));
                break;
            }
            if chosen.is_none() && !n.contains("Midi Through") && !n.contains("System") {
                chosen = Some((i, n));
            }
        }
    }
    let Some((idx, name)) = chosen else {
        eprintln!("⚠️ Aucun clavier MIDI détecté — reconnaissance d'accords désactivée");
        return;
    };
    let shared2 = Arc::clone(shared);
    let conn = mi.connect(
        &ports[idx],
        "chords-server-rs-in",
        move |_, msg, _| {
            if let Ok(mut a) = shared2.active.lock() {
                apply_midi_message(&mut a, msg);
            }
        },
        (),
    );
    match conn {
        Ok(c) => {
            *shared.device.lock().unwrap() = Some(name.clone());
            *shared._conn.lock().unwrap() = Some(c);
            println!("🎹 Reconnaissance d'accords : écoute de « {} »", name);
        }
        Err(e) => eprintln!("⚠️ Connexion entrée MIDI échouée : {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(active: &mut Vec<u8>, status: u8, d1: u8, d2: u8) {
        apply_midi_message(active, &[status, d1, d2]);
    }

    #[test]
    fn note_on_ajoute_dans_l_ordre_d_arrivee() {
        let mut a = Vec::new();
        send(&mut a, 0x90, 67, 100); // G d'abord
        send(&mut a, 0x90, 60, 100); // puis C
        send(&mut a, 0x90, 64, 100); // puis E
        // L'ordre d'appui du pianiste est CONSERVÉ (pas de tri) :
        assert_eq!(a, vec![67, 60, 64]);
    }

    #[test]
    fn doublure_pas_ajoutee_deux_fois() {
        let mut a = Vec::new();
        send(&mut a, 0x90, 60, 100);
        send(&mut a, 0x90, 60, 100); // même touche re-jouée (repiquage)
        assert_eq!(a, vec![60]);
    }

    #[test]
    fn note_off_et_note_on_vel_0_retirent() {
        let mut a = vec![60, 64, 67];
        send(&mut a, 0x80, 64, 64); // note-off classique
        assert_eq!(a, vec![60, 67]);
        send(&mut a, 0x90, 67, 0); // note-on vel 0 = note-off (certains claviers)
        assert_eq!(a, vec![60]);
    }

    #[test]
    fn canal_drums_ignore() {
        let mut a = Vec::new();
        send(&mut a, 0x99, 36, 100); // canal 9 drums
        assert_eq!(a, Vec::<u8>::new());
        // Les autres canaux (dont le 0 du piano) sont pris en compte
        send(&mut a, 0x90, 60, 100);
        assert_eq!(a, vec![60]);
    }

    #[test]
    fn messages_non_note_ignores() {
        let mut a = Vec::new();
        apply_midi_message(&mut a, &[0xB0, 7, 100]); // CC volume
        apply_midi_message(&mut a, &[0xC0, 5]); // program change (2 octets)
        apply_midi_message(&mut a, &[0x90]); // message trop court
        assert_eq!(a, Vec::<u8>::new());
    }

    #[test]
    fn release_dune_note_non_jouee_est_sans_effet() {
        let mut a = vec![60];
        send(&mut a, 0x80, 72, 64); // note jamais jouée
        assert_eq!(a, vec![60]);
    }
}
