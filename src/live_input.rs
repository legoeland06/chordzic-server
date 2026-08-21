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

use crate::mpe::{self, MpeState};

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
    /// Événements d'expression MPE horodatés (gestes de la modal 🎛).
    pub expr: Vec<mpe::RecExpr>,
}

impl RecSession {
    pub fn new() -> Self {
        Self {
            start: std::time::Instant::now(),
            notes: Vec::new(),
            held: std::collections::HashMap::new(),
            expr: Vec::new(),
        }
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

/// Canal cible des modulations MPE : canal de l'écho ✨ si actif, sinon canal
/// explicite de la modal, sinon canal 0 — le canal de jeu 1 du pianiste
/// (0-indexé, convention de tout le code : pistes 0/2/3/4/9, piano-note 0).
/// Fonction pure (testable sans matériel).
pub fn resolve_mpe_channel(mpe: &MpeState, echo: &EchoConfig) -> u8 {
    if echo.enabled {
        if let Some(ch) = echo.channel {
            return ch;
        }
    }
    mpe.channel.unwrap_or(0)
}

/// Monitoring (écho ✨ / modal MPE / THRU par défaut) : quand l'utilisateur
/// joue sur le Roland (Local Control OFF), le serveur lui renvoie TOUJOURS
/// ses notes — c'est le comportement historique « le serveur renvoie du son
/// au Roland » — l'écho ✨ change le canal (piste + program), la modal MPE
/// n'ajoute que les modulations.
/// Fonction pure (testable sans matériel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputTarget {
    Main,
    Fluid,
}

pub fn monitor_output_target(mpe: &MpeState, echo: &EchoConfig) -> OutputTarget {
    if mpe.enabled && mpe.target == mpe::MpeTarget::Fluid {
        return OutputTarget::Fluid;
    }
    // Écho ✨, monitoring MPE ou thru par défaut : sortie principale.
    OutputTarget::Main
}

/// Monitoring (écho ✨ / modal MPE / thru par défaut) : les notes du
/// pianiste et la pédale de sustain sont renvoyées sur le canal cible —
/// canal de l'écho ✨ si actif, sinon canal MPE explicite, sinon canal 0
/// (canal de jeu). Retourne None pour les messages non relayés (CC autres,
/// PC…). Fonction pure (testable sans matériel).
pub fn monitor_message(msg: &[u8], mpe: &MpeState, echo: &EchoConfig) -> Option<Vec<u8>> {
    // Écho ✨ prioritaire : il relaie déjà notes + sustain sur le canal piste.
    if echo.enabled {
        return echo_message(msg, echo);
    }
    if msg.len() < 2 {
        return None;
    }
    let channel = resolve_mpe_channel(mpe, echo);
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

/// Messages d'expression MPE à envoyer sur le canal cible (bend effectif LFO
/// inclus, aftertouch, timbre) — appelé après chaque note relayée pour que la
/// modulation s'applique dès l'appui, et par le ticker LFO en continu.
/// `t_ms` : instant courant (pour la phase du LFO). Fonction pure.
pub fn mpe_expression_out(mpe: &MpeState, echo: &EchoConfig, t_ms: u64) -> Vec<Vec<u8>> {
    if !mpe.enabled {
        return Vec::new();
    }
    let ch = resolve_mpe_channel(mpe, echo);
    vec![
        mpe::pitch_bend_message(ch, mpe::effective_bend(mpe, t_ms)),
        mpe::channel_pressure_message(ch, mpe.pressure),
        mpe::timbre_message(ch, mpe.timbre),
    ]
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
    /// Arc : partagée avec le thread de lecture (play-along rec_after_beats).
    pub rec: Arc<Mutex<Option<RecSession>>>,
    /// Compensation de latence MIDI IN (ms) appliquée à l'horodatage des
    /// notes enregistrées : le serveur horodate à la RÉCEPTION du message ;
    /// la note a été jouée ~comp ms plus tôt (transport USB MIDI + pile
    /// ALSA, constant pour un matériel donné — env REC_COMP_MS, défaut 2).
    pub rec_comp_ms: std::sync::atomic::AtomicU64,
    /// Émetteur MIDI (configuré par main.rs avec la sortie réelle).
    sender: Mutex<Option<Box<dyn Fn(&[u8]) + Send + Sync>>>,
    /// Émetteur MIDI vers FluidSynth (cible « PC » de la modal MPE) —
    /// connexion séparée de la sortie principale (le métronome en a déjà
    /// une, midir permet plusieurs connexions au même port).
    pub(crate) fluid_sender: Mutex<Option<Box<dyn Fn(&[u8]) + Send + Sync>>>,
    /// État MPE (modal 🎛) : monitoring des notes + modulations d'expression
    /// (bend / aftertouch / timbre) injectées dans le flux renvoyé au clavier.
    pub mpe: Mutex<MpeState>,
}

impl LiveInputState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            device: Mutex::new(None),
            active: Mutex::new(Vec::new()),
            _conn: Mutex::new(None),
            echo: Mutex::new(EchoConfig::default()),
            sustain: std::sync::atomic::AtomicBool::new(false),
            rec: Arc::new(Mutex::new(None)),
            rec_comp_ms: std::sync::atomic::AtomicU64::new(2),
            sender: Mutex::new(None),
            fluid_sender: Mutex::new(None),
            mpe: Mutex::new(MpeState::default()),
        })
    }
}

/// Configure l'émetteur MIDI utilisé par l'écho (la sortie réelle,
/// gérée par main.rs — reconnexion automatique incluse).
pub fn set_echo_sender(shared: &Arc<LiveInputState>, sender: Box<dyn Fn(&[u8]) + Send + Sync>) {
    *shared.sender.lock().unwrap() = Some(sender);
}

/// Configure l'émetteur MIDI vers FluidSynth (cible « PC » de la modal MPE).
/// None côté main.rs si FluidSynth est indisponible → repli sur la sortie
/// principale (voir `monitor_output_target`).
pub fn set_fluid_sender(shared: &Arc<LiveInputState>, sender: Box<dyn Fn(&[u8]) + Send + Sync>) {
    *shared.fluid_sender.lock().unwrap() = Some(sender);
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
            // Écho vers la sortie (mode Navig ✨) OU monitoring MPE (modal 🎛) :
            // le pianiste entend le son renvoyé (notes + pédale) — avec le
            // program de la piste (écho) ou le son du canal cible (MPE).
            // Copies des états : les verrous ne sont JAMAIS imbriqués avec
            // d'autres et sont relâchés avant tout envoi (le ticker LFO et la
            // route /mpe prennent les mêmes verrous — un seul ordre global :
            // echo PUIS mpe — pas de deadlock possible).
            // ⚠️ AUCUN unwrap() ici : ce callback tourne sur le thread MIDI IN
            // (midir) — s'il panique, le thread meurt et le LivePiano se fige
            // (l'état active n'est plus mis à jour) alors que le serveur HTTP
            // continue de répondre. Un verrou empoisonné → on ignore le
            // message et on survit.
            let (echo_cfg, mpe_state) = {
                let Ok(cfg) = shared2.echo.lock() else { return; };
                let Ok(m) = shared2.mpe.lock() else { return; };
                (*cfg, *m)
            };
            let route = monitor_output_target(&mpe_state, &echo_cfg);
            // Thru par défaut : les notes du pianiste sont TOUJOURS renvoyées
            // (écho ✨ sur le canal piste, sinon canal de jeu) — la modal MPE
            // ajoute les modulations (bend effectif LFO inclus, aftertouch,
            // timbre) dès l'appui. Cible : FluidSynth si la modal le force.
            let out = monitor_message(msg, &mpe_state, &echo_cfg);
            let exprs = if mpe_state.enabled {
                let t = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                mpe_expression_out(&mpe_state, &echo_cfg, t)
            } else {
                Vec::new()
            };
            let fluid = route == OutputTarget::Fluid;
            let sender_guard = if fluid {
                match shared2.fluid_sender.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                }
            } else {
                match shared2.sender.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                }
            };
            if let Some(sender) = sender_guard.as_ref() {
                if let Some(m) = out {
                    sender(&m);
                }
                for expr in &exprs {
                    sender(expr);
                }
            }
            // Enregistrement MIDI (mode Navig, Rec) : les notes jouées sont
            // horodatées depuis le début de la session, AVANCÉES de la
            // latence MIDI IN (compensation constante du transport) — le
            // placement dans le piano roll colle à l'appui réel, pas à la
            // réception serveur.
            if let Ok(mut rec) = shared2.rec.lock() {
                if let Some(session) = rec.as_mut() {
                    let comp = shared2.rec_comp_ms.load(std::sync::atomic::Ordering::Relaxed) as u64;
                    let now = session.now_ms().saturating_sub(comp);
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

    // ── MPE : monitoring + modulations ──

    fn mpe_on(channel: Option<u8>) -> MpeState {
        MpeState { enabled: true, channel, ..Default::default() }
    }

    #[test]
    fn resolve_canal_echo_puis_mpe_puis_0() {
        let echo_on = EchoConfig { enabled: true, channel: Some(5) };
        let echo_off = EchoConfig { enabled: false, channel: None };
        // Écho actif → canal de la piste
        assert_eq!(resolve_mpe_channel(&mpe_on(None), &echo_on), 5);
        // Pas d'écho → canal explicite de la modal
        assert_eq!(resolve_mpe_channel(&mpe_on(Some(3)), &echo_off), 3);
        // Rien → canal 0 (le canal de jeu 1 du pianiste, 0-indexé)
        assert_eq!(resolve_mpe_channel(&mpe_on(None), &echo_off), 0);
        // MPE désactivé → canal 0 par défaut
        assert_eq!(resolve_mpe_channel(&MpeState::default(), &echo_off), 0);
    }

    #[test]
    fn monitor_sans_echo_relaie_notes_et_sustain_sur_canal_mpe() {
        let echo_off = EchoConfig::default();
        let m = mpe_on(None); // canal cible = 0 (canal de jeu 1)
        let on = monitor_message(&[0x90, 60, 100], &m, &echo_off).unwrap();
        assert_eq!(on, vec![0x90, 60, 100]); // canal 0 (inchangé)
        let off = monitor_message(&[0x80, 64, 64], &m, &echo_off).unwrap();
        assert_eq!(off, vec![0x80, 64, 64]);
        let sus = monitor_message(&[0xB0, 64, 127], &m, &echo_off).unwrap();
        assert_eq!(sus, vec![0xB0, 64, 127]);
        // Canal explicite de la modal : remappé
        let m2 = mpe_on(Some(2));
        let on2 = monitor_message(&[0x90, 60, 100], &m2, &echo_off).unwrap();
        assert_eq!(on2, vec![0x92, 60, 100]);
    }

    #[test]
    fn monitor_mpe_desactive_relaie_quand_meme_thru_par_defaut() {
        let echo_off = EchoConfig::default();
        // MPE désactivé + pas d'écho → THRU par défaut : les notes reviennent
        // sur le canal de jeu (comportement historique « le serveur renvoie
        // du son au Roland »).
        let off = MpeState::default();
        let on = monitor_message(&[0x90, 60, 100], &off, &echo_off).unwrap();
        assert_eq!(on, vec![0x90, 60, 100]);
        // Les messages non relayés (CC volume, PC, trop court) → None
        assert_eq!(monitor_message(&[0xB0, 7, 100], &off, &echo_off), None);
        assert_eq!(monitor_message(&[0xC0, 5], &off, &echo_off), None);
        assert_eq!(monitor_message(&[0x90], &off, &echo_off), None);
    }

    #[test]
    fn monitor_echo_prioritaire_sur_mpe() {
        let echo_on = EchoConfig { enabled: true, channel: Some(5) };
        let m = mpe_on(None);
        let out = monitor_message(&[0x90, 60, 100], &m, &echo_on).unwrap();
        assert_eq!(out, vec![0x95, 60, 100]); // canal de la piste, pas le MPE
    }

    #[test]
    fn expression_out_envoie_bend_at_timbre_sur_le_bon_canal() {
        let echo_off = EchoConfig::default();
        let mut m = mpe_on(None);
        m.bend = 9000;
        m.pressure = 80;
        m.timbre = 30;
        let out = mpe_expression_out(&m, &echo_off, 0);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], vec![0xE0, (9000 & 0x7F) as u8, ((9000 >> 7) & 0x7F) as u8]); // bend canal 0
        assert_eq!(out[1], vec![0xD0, 80]); // aftertouch
        assert_eq!(out[2], vec![0xB0, 74, 30]); // timbre
    }

    #[test]
    fn expression_out_sur_canal_de_echo() {
        let echo_on = EchoConfig { enabled: true, channel: Some(7) };
        let m = mpe_on(None);
        let out = mpe_expression_out(&m, &echo_on, 0);
        assert_eq!(out[0][0], 0xE7); // bend sur le canal 7
        assert_eq!(out[1][0], 0xD7);
        assert_eq!(out[2][0], 0xB7);
    }

    #[test]
    fn expression_out_desactive_si_mpe_off() {
        let echo_off = EchoConfig::default();
        let off = MpeState::default();
        assert_eq!(mpe_expression_out(&off, &echo_off, 0), Vec::<Vec<u8>>::new());
    }

    // ── Routage de sortie (écho ✨ / monitoring MPE / FluidSynth) ──

    fn mpe_auto(channel: Option<u8>) -> MpeState {
        MpeState { enabled: true, channel, ..Default::default() }
    }

    fn mpe_fluid(channel: Option<u8>) -> MpeState {
        MpeState { enabled: true, target: mpe::MpeTarget::Fluid, channel, ..Default::default() }
    }

    #[test]
    fn routage_echo_prioritaire_sans_modal() {
        // Comportement HISTORIQUE : écho ✨ actif → sortie principale, même
        // sans modal MPE (régression couverte par test).
        let echo_on = EchoConfig { enabled: true, channel: Some(2) };
        assert_eq!(monitor_output_target(&MpeState::default(), &echo_on), OutputTarget::Main);
        assert_eq!(monitor_output_target(&mpe_auto(None), &echo_on), OutputTarget::Main);
    }

    #[test]
    fn routage_monitoring_mpe_sans_echo() {
        let echo_off = EchoConfig::default();
        // Modal ouverte (auto) → sortie principale (Roland)
        assert_eq!(monitor_output_target(&mpe_auto(None), &echo_off), OutputTarget::Main);
        // Modal fermée → THRU par défaut : toujours la sortie principale
        assert_eq!(monitor_output_target(&MpeState::default(), &echo_off), OutputTarget::Main);
    }

    #[test]
    fn routage_fluid_force_le_pc() {
        let echo_on = EchoConfig { enabled: true, channel: Some(2) };
        let echo_off = EchoConfig::default();
        // Cible fluid → FluidSynth, même avec l'écho ✨ actif (pas de double son)
        assert_eq!(monitor_output_target(&mpe_fluid(None), &echo_on), OutputTarget::Fluid);
        assert_eq!(monitor_output_target(&mpe_fluid(None), &echo_off), OutputTarget::Fluid);
        // Modal fermée mais cible fluid → l'écho ✨ / thru reprennent la main
        let mut off = MpeState::default();
        off.target = mpe::MpeTarget::Fluid;
        assert_eq!(monitor_output_target(&off, &echo_on), OutputTarget::Main);
        assert_eq!(monitor_output_target(&off, &echo_off), OutputTarget::Main);
    }

    #[test]
    fn thru_par_defaut_sur_le_canal_de_jeu() {
        let echo_off = EchoConfig::default();
        let off = MpeState::default();
        // Note reçue sur le canal 0 → renvoyée telle quelle (canal de jeu)
        assert_eq!(monitor_message(&[0x90, 60, 100], &off, &echo_off), Some(vec![0x90, 60, 100]));
        assert_eq!(monitor_message(&[0x80, 64, 64], &off, &echo_off), Some(vec![0x80, 64, 64]));
        // Note venue d'un autre canal → remappée sur le canal de jeu (0)
        assert_eq!(monitor_message(&[0x93, 60, 100], &off, &echo_off), Some(vec![0x90, 60, 100]));
        // Canal MPE explicite prioritaire sur le canal d'origine
        let m = mpe_on(Some(4));
        assert_eq!(monitor_message(&[0x90, 60, 100], &m, &echo_off), Some(vec![0x94, 60, 100]));
    }

    #[test]
    fn routage_roland_explicite() {
        let echo_on = EchoConfig { enabled: true, channel: Some(2) };
        let mut m = mpe_auto(None);
        m.target = mpe::MpeTarget::Roland;
        // Cible roland explicite → sortie principale (l'écho reste la cible
        // du canal mais le son sort du même port principal)
        assert_eq!(monitor_output_target(&m, &echo_on), OutputTarget::Main);
        assert_eq!(monitor_output_target(&m, &EchoConfig::default()), OutputTarget::Main);
    }
}
