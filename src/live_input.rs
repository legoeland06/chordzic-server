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

/// Configuration de l'écho des notes du pianiste vers la sortie MIDI
/// (mode Navig, toggle ✨) : le Roland joue avec le son (program) de la
/// piste sélectionnée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EchoConfig {
    /// Écho activé (piste sélectionnée + bouton ✨ ON).
    pub enabled: bool,
    /// Canal de la piste cible (None = aucune piste sélectionnée).
    pub channel: Option<u8>,
}

impl Default for EchoConfig {
    fn default() -> Self {
        Self { enabled: false, channel: None }
    }
}

// ── Enregistrement MIDI (mode Navig, Rec MIDI) ─────────────────────────

/// Note enregistrée pendant une session Rec MIDI.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RecNote {
    pub pitch: u8,
    /// ms depuis le début de la session (note-on).
    pub on_ms: u64,
    /// ms de la note-off (None = encore tenue).
    pub off_ms: Option<u64>,
}

/// Session d'enregistrement MIDI : accumulateur d'événements horodatés.
pub struct RecSession {
    /// Instant de début (référence des timestamps).
    pub start: std::time::Instant,
    /// Notes en cours d'enregistrement (ordre d'appui conservé).
    pub notes: Vec<RecNote>,
    /// pitch → index dans `notes` (retrouver la note à la relâche).
    pub held: std::collections::HashMap<u8, usize>,
}

impl RecSession {
    pub fn new() -> Self {
        Self { start: std::time::Instant::now(), notes: Vec::new(), held: std::collections::HashMap::new() }
    }

    /// ms écoulées depuis le début de la session.
    pub fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
}

/// Applique un message de note (note-on/off) à la session d'enregistrement.
/// Fonction pure (testable sans matériel) :
/// - note-on (vel > 0) → nouvelle note (repiquage sur une note tenue ignoré) ;
/// - note-off / note-on vel 0 → ferme la durée de la note tenue ;
/// - autres messages (CC, PC…) → ignorés.
pub fn apply_rec_message(
    notes: &mut Vec<RecNote>,
    held: &mut std::collections::HashMap<u8, usize>,
    msg: &[u8],
    now_ms: u64,
) {
    if msg.len() < 2 {
        return;
    }
    let high = msg[0] & 0xF0;
    let pitch = msg[1];
    let is_on = match high {
        0x90 => msg.get(2).copied().unwrap_or(127) > 0,
        0x80 => false,
        _ => return,
    };
    if is_on {
        if !held.contains_key(&pitch) {
            held.insert(pitch, notes.len());
            notes.push(RecNote { pitch, on_ms: now_ms, off_ms: None });
        }
    } else if let Some(&idx) = held.get(&pitch) {
        if notes[idx].off_ms.is_none() {
            notes[idx].off_ms = Some(now_ms);
        }
        held.remove(&pitch);
    }
}

/// Transforme un message pour l'écho vers la piste cible : le canal est
/// remplacé par celui de la piste. Sont relayés les NOTES (note-on/off) et
/// la PÉDALE DE SUSTAIN (CC64) — les autres CC/PC/pitch bend ne sont pas
/// échoés. Retourne None si l'écho est désactivé, sans piste cible, ou si
/// le message n'est pas relégué.
/// Fonction pure (testable sans matériel).
pub fn echo_message(msg: &[u8], echo: &EchoConfig) -> Option<Vec<u8>> {
    if !echo.enabled || msg.len() < 2 {
        return None;
    }
    let channel = echo.channel?;
    let high = msg[0] & 0xF0;
    let is_note = high == 0x90 || high == 0x80;
    let is_sustain = high == 0xB0 && msg.len() >= 3 && msg[1] == 64;
    if !is_note && !is_sustain {
        return None;
    }
    let mut out = msg.to_vec();
    out[0] = high | (channel & 0x0F);
    Some(out)
}

/// Messages à envoyer pour une note jouée au clic sur le LivePiano :
/// note-on (vel > 0) ou note-off (0x80) sur le canal cible.
/// Fonction pure (testable sans matériel).
pub fn piano_note_message(pitch: u8, velocity: u8, on: bool, channel: u8) -> Vec<u8> {
    if on {
        vec![0x90 | (channel & 0x0F), pitch, velocity.min(127)]
    } else {
        vec![0x80 | (channel & 0x0F), pitch, 0]
    }
}

/// Messages à envoyer pour faire sonner une piste avec son instrument :
/// program change (ou bank select + PC pour les banques GM2). Le canal
/// drums natif (9) n'a pas de PC (kit standard) sauf banque explicite.
/// Fonction pure (testable sans matériel).
pub fn program_change_messages(
    out_channel: u8,
    program: u8,
    bank_msb: u8,
    bank_lsb: u8,
) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if out_channel != 9 {
        out.push(vec![0xC0 | out_channel, program]);
    } else if bank_msb != 0 || bank_lsb != 0 {
        out.push(vec![0xB0 | 9, 0, bank_msb]);
        out.push(vec![0xB0 | 9, 32, bank_lsb]);
        out.push(vec![0xC0 | 9, program]);
    }
    out
}

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
    /// Écho des notes du pianiste vers la sortie MIDI (mode Navig ✨).
    pub echo: Mutex<EchoConfig>,
    /// État courant de la pédale de sustain (CC64) — relancé à l'activation
    /// de l'écho pour que le sustain s'applique aussi aux notes renvoyées.
    pub sustain: std::sync::atomic::AtomicBool,
    /// Session d'enregistrement MIDI (mode Navig, Rec) — None si inactive.
    pub rec: Mutex<Option<RecSession>>,
    /// Émetteur MIDI (configuré par main.rs avec la sortie réelle).
    sender: Mutex<Option<Box<dyn Fn(&[u8]) + Send + Sync>>>,
}

impl LiveInputState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            device: Mutex::new(None),
            active: Mutex::new(Vec::new()),
            _conn: Mutex::new(None),
            echo: Mutex::new(EchoConfig::default()),
            sustain: std::sync::atomic::AtomicBool::new(false),
            rec: Mutex::new(None),
            sender: Mutex::new(None),
        })
    }
}

/// Configure l'émetteur MIDI utilisé par l'écho (la sortie réelle,
/// gérée par main.rs — reconnexion automatique incluse).
pub fn set_echo_sender(shared: &Arc<LiveInputState>, sender: Box<dyn Fn(&[u8]) + Send + Sync>) {
    *shared.sender.lock().unwrap() = Some(sender);
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
            // Pédale de sustain : mémorise l'état courant (relancé à
            // l'activation de l'écho) puis écho vers la sortie.
            if msg.len() >= 3 && (msg[0] & 0xF0) == 0xB0 && msg[1] == 64 {
                shared2.sustain.store(msg[2] >= 64, std::sync::atomic::Ordering::Relaxed);
            }
            // Écho vers la sortie (mode Navig ✨) : le pianiste entend le son
            // (program) de la piste sélectionnée sur le canal de celle-ci —
            // notes ET pédale de sustain (les notes écho durent avec la pédale).
            let cfg = shared2.echo.lock().unwrap();
            if let Some(out) = echo_message(msg, &cfg) {
                if let Some(sender) = shared2.sender.lock().unwrap().as_ref() {
                    sender(&out);
                }
            }
            // Enregistrement MIDI (mode Navig, Rec) : les notes jouées sont
            // horodatées depuis le début de la session.
            if let Ok(mut rec) = shared2.rec.lock() {
                if let Some(session) = rec.as_mut() {
                    let now = session.now_ms();
                    apply_rec_message(&mut session.notes, &mut session.held, msg, now);
                }
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

    // ── Écho (mode Navig ✨) ──

    #[test]
    fn echo_remplace_le_canal_de_la_note() {
        let cfg = EchoConfig { enabled: true, channel: Some(5) };
        let out = echo_message(&[0x90, 60, 100], &cfg).unwrap();
        assert_eq!(out, vec![0x95, 60, 100]); // canal 0 → 5, note intacte
        let off = echo_message(&[0x80, 64, 64], &cfg).unwrap();
        assert_eq!(off, vec![0x85, 64, 64]);
    }

    #[test]
    fn echo_relaie_la_pedale_de_sustain() {
        let cfg = EchoConfig { enabled: true, channel: Some(2) };
        let on = echo_message(&[0xB0, 64, 127], &cfg).unwrap();
        assert_eq!(on, vec![0xB2, 64, 127]); // CC64 canal 0 → 2
        let off = echo_message(&[0xB0, 64, 0], &cfg).unwrap();
        assert_eq!(off, vec![0xB2, 64, 0]);
    }

    #[test]
    fn echo_desactive_sans_piste_ou_sur_message_non_relaye() {
        let off = EchoConfig { enabled: false, channel: Some(0) };
        assert_eq!(echo_message(&[0x90, 60, 100], &off), None);
        let no_ch = EchoConfig { enabled: true, channel: None };
        assert_eq!(echo_message(&[0x90, 60, 100], &no_ch), None);
        let on = EchoConfig { enabled: true, channel: Some(0) };
        assert_eq!(echo_message(&[0xB0, 7, 100], &on), None); // CC volume
        assert_eq!(echo_message(&[0xC0, 5], &on), None); // program change
        assert_eq!(echo_message(&[0x90], &on), None); // trop court
    }

    #[test]
    fn program_change_simple_sur_canal_non_drums() {
        let msgs = program_change_messages(2, 51, 0, 0);
        assert_eq!(msgs, vec![vec![0xC2, 51]]);
    }

    #[test]
    fn program_change_drums_avec_banque() {
        let msgs = program_change_messages(9, 128, 1, 0);
        assert_eq!(msgs, vec![vec![0xB9, 0, 1], vec![0xB9, 32, 0], vec![0xC9, 128]]);
    }

    // ── Enregistrement MIDI (Rec) ──

    fn rec_send(notes: &mut Vec<RecNote>, held: &mut std::collections::HashMap<u8, usize>, status: u8, d1: u8, d2: u8, now: u64) {
        apply_rec_message(notes, held, &[status, d1, d2], now);
    }

    #[test]
    fn rec_note_on_cree_et_off_ferme_la_duree() {
        let mut notes = Vec::new();
        let mut held = std::collections::HashMap::new();
        rec_send(&mut notes, &mut held, 0x90, 60, 100, 0);   // C4 à t=0
        rec_send(&mut notes, &mut held, 0x90, 64, 100, 500); // E4 à t=500ms
        rec_send(&mut notes, &mut held, 0x80, 60, 64, 1000); // C4 relâché
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0], RecNote { pitch: 60, on_ms: 0, off_ms: Some(1000) });
        assert_eq!(notes[1], RecNote { pitch: 64, on_ms: 500, off_ms: None });
        assert!(!held.contains_key(&60));
        assert!(held.contains_key(&64));
    }

    #[test]
    fn rec_conserve_l_ordre_d_appui() {
        let mut notes = Vec::new();
        let mut held = std::collections::HashMap::new();
        rec_send(&mut notes, &mut held, 0x90, 67, 100, 0);
        rec_send(&mut notes, &mut held, 0x90, 60, 100, 50);
        rec_send(&mut notes, &mut held, 0x90, 64, 100, 100);
        assert_eq!(notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![67, 60, 64]);
    }

    #[test]
    fn rec_repiquage_dune_note_tenue_ignore() {
        let mut notes = Vec::new();
        let mut held = std::collections::HashMap::new();
        rec_send(&mut notes, &mut held, 0x90, 60, 100, 0);
        rec_send(&mut notes, &mut held, 0x90, 60, 100, 100); // repiquage
        assert_eq!(notes.len(), 1);
    }

    #[test]
    fn piano_note_on_off() {
        // note-on : status 0x90 + canal, pitch, vélocité (bornée à 127)
        assert_eq!(piano_note_message(60, 100, true, 0), vec![0x90, 60, 100]);
        assert_eq!(piano_note_message(60, 100, true, 9), vec![0x99, 60, 100]);
        assert_eq!(piano_note_message(60, 200, true, 2), vec![0x92, 60, 127]); // vélocité bornée
        // note-off : 0x80 + canal, pitch, 0
        assert_eq!(piano_note_message(60, 100, false, 0), vec![0x80, 60, 0]);
        assert_eq!(piano_note_message(60, 100, false, 15), vec![0x8F, 60, 0]);
    }

    #[test]
    fn rec_note_on_vel_0_ferme_et_non_note_ignore() {
        let mut notes = Vec::new();
        let mut held = std::collections::HashMap::new();
        rec_send(&mut notes, &mut held, 0x90, 60, 100, 0);
        rec_send(&mut notes, &mut held, 0x90, 60, 0, 200); // note-on vel 0 = off
        assert_eq!(notes[0].off_ms, Some(200));
        rec_send(&mut notes, &mut held, 0xB0, 7, 100, 300); // CC ignoré
        rec_send(&mut notes, &mut held, 0x90, 61, 0, 400); // off d'une note jamais jouée
        assert_eq!(notes.len(), 1);
    }

    #[test]
    fn program_change_drums_sans_banque_vide() {
        assert_eq!(program_change_messages(9, 1, 0, 0), Vec::<Vec<u8>>::new());
    }
}
