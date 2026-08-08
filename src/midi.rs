/// MIDI temps réel — LiveTrack, Live, play_seq, drum_hit, helpers MIDI.
///
/// Ce module gère la communication MIDI en temps réel via midir.
/// Il envoie les notes MIDI vers FluidSynth (ou un synthé hardware)
/// pour une lecture live synchronisée avec la grille d'accords.
///
/// Architecture :
/// - `LiveTrack`   : configuration mutable d'une piste (program, volume, mute)
/// - `Live`        : état global partagé entre les threads (tempo, pattern, mutes...)
/// - `play_seq()`  : boucle principale de lecture séquentielle des accords
/// - `drum_hit()`  : génère les coups de batterie selon le pattern sélectionné
///
/// Contraintes temps réel :
/// - Tout partage d'état se fait via Atomic* pour éviter les locks sur le chemin
///   critique (le thread audio ne doit pas être bloqué).
/// - Le thread audio tourne en boucle serrée avec sleep précis (delay_ms ≈ 60ms
///   à 100 BPM pour une résolution en 16ème de note ~= 30ms min).
use midir::{MidiOutput, MidiOutputConnection};
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use crate::patterns::{sc, DRUM_HH, DRUM_HH_OPEN, DRUM_KICK, DRUM_RIDE, DRUM_RIM, DRUM_SNARE, HH_8TH, HH_BEAT, PAT_BOSSA, PAT_JAZZ, PAT_ONEDROP, PAT_POP, PAT_REGGAE};
use crate::walking::{generate_walking_bass, is_minor, MIN_NOTE};

/// Handle MIDI partagé entre threads — wrapping atomique de la connection midir.
pub type MidiHandle = Arc<Mutex<MidiOutputConnection>>;

// ─── Rôles des pistes (identifiés par canal MIDI) ─────────────────────
// Les rôles classiques sont sur les canaux 0 (lead), 2 (bass), 3 (nappes),
// 9 (drums) et 4 (accent). Les pistes sont DYNAMIQUES : l'utilisateur peut
// ajouter/supprimer des pistes — un rôle absent est simplement muet
// (lookup par canal dans play_seq / generate_notes).

/// Configuration mutable d'une piste MIDI.
/// Chaque champ est atomique pour permettre la modification depuis
/// le thread HTTP sans bloquer le thread audio.
pub struct LiveTrack {
    pub channel: u8,              // Canal MIDI (0-15, 9 = drums)
    pub program: AtomicU16,       // Program change (instrument GM)
    pub volume: AtomicU8,         // Volume de la piste (0-127)
    pub mute: AtomicBool,         // Mute — si true, la piste ne joue pas
    /// Reverb send (CC91) appliqué en live. 0-100.
    pub fx_reverb: AtomicU8,
    /// Chorus send (CC93) appliqué en live. 0-100.
    pub fx_chorus: AtomicU8,
    /// Piste percussion (kit drums) sur un canal ≠ 9 : programmée via la
    /// banque percussion GM2 (bank select 128) + kit Standard (program 0).
    pub drums: AtomicBool,
}

impl LiveTrack {
    pub fn new(ch: u8, pg: u16, vol: u8) -> Self {
        Self {
            channel: ch,
            program: AtomicU16::new(pg),
            volume: AtomicU8::new(vol),
            mute: AtomicBool::new(false),
            fx_reverb: AtomicU8::new(0),
            fx_chorus: AtomicU8::new(0),
            drums: AtomicBool::new(false),
        }
    }
}

/// État global de la lecture live, partagé entre le thread HTTP (configuration)
/// et le thread audio (lecture séquentielle).
///
/// Tous les champs sont atomiques ou Mutex pour un accès thread-safe
/// sans contention significative.
pub struct Live {
    /// Pistes MIDI — DYNAMIQUE : l'utilisateur peut ajouter/supprimer des
    /// pistes (chaque piste = un canal MIDI avec instrument, volume, mute).
    /// Mutex : modifiable depuis les handlers HTTP (ajout/suppression),
    /// lu une fois au début de play_seq (références gardées sous le guard).
    pub tracks: Mutex<Vec<LiveTrack>>,
    pub pattern: AtomicU8,                // Pattern de batterie (PAT_ROCK, etc.)
    pub tempo: AtomicU16,                 // BPM actuel
    pub stop: AtomicBool,                 // Flag d'arrêt — thread audio vérifie à chaque beat
    pub sig: AtomicU16,                   // Signature rythmique encodée (ex: 44 = 4/4)
    pub walking: AtomicBool,              // Walking bass activée/désactivée
    pub master_vol: AtomicU8,             // Master volume global (0-127)
    pub use432: AtomicBool,               // Accordage 432Hz au lieu de 440Hz
}

// ─── Note MIDI ──────────────────────────────────────────────────────────

/// Convertit un nom de note textuel (ex: "C4", "F#3", "Bb5") en note MIDI (0-127).
///
/// Format accepté : `[Note][Alt][Octave]`
/// - Note : A, B, C, D, E, F, G (insensible à la casse)
/// - Alt (optionnel) : # (dièse) ou b (bémol). Les bémols doublés (Db → C#, etc.)
///   sont automatiquement convertis en dièse.
/// - Octave : 0-10 (MIDI standard : C4 = 60)
///
/// # Exemples
/// - "C4"  → 60  (Do central)
/// - "F#3" → 54  (Fa# de l'octave 3)
/// - "Bb4" → 70  (Sib = La# de l'octave 4)
pub fn note_midi(s: &str) -> Result<u8, String> {
    let s = s.trim();
    // Détecter si la note a 1 ou 2 caractères de nom (ex: "C" vs "C#")
    let (nl, np) = if s.len() > 1 && (s.as_bytes()[1] == b'#' || s.as_bytes()[1] == b'b') {
        (2, &s[..2]) // Note + alt : "F#", "Bb"
    } else {
        (1, &s[..1]) // Note seule : "C", "A"
    };
    let o: i32 = s[nl..].parse().map_err(|_| "octave invalide")?;
    let u = np.to_uppercase();
    // Normaliser les bémols en dièses (Db → C#, etc.)
    let n = match u.as_str() {
        "DB" => "C#", "EB" => "D#", "GB" => "F#", "AB" => "G#", "BB" => "A#",
        _ => &u,
    };
    // Tableau des 12 notes chromatiques (index = demi-ton depuis C)
    let i = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
        .iter()
        .position(|x| x == &n)
        .ok_or("note inconnue")?;
    // MIDI = (octave + 1) * 12 + index chromatique
    // Ex: C4 = (4+1)*12 + 0 = 60
    let m = (o + 1) * 12 + i as i32;
    if m < 0 || m > 127 { return Err("note hors limites MIDI".into()) }
    Ok(m as u8)
}

/// Convertit un vecteur de noms de notes textuels en notes MIDI.
/// Ignore silencieusement les notes invalides.
pub fn notes_from_vec(notes: &[String]) -> Vec<u8> {
    let mut v = vec![];
    for n in notes {
        if let Ok(x) = note_midi(n) { v.push(x) }
    }
    v
}

// ─── Init MIDI ──────────────────────────────────────────────────────────

/// Initialise la connexion MIDI de sortie.
///
/// Lis la variable d'environnement `MIDI_PORT` pour choisir le port.
/// Sur Linux : défaut port 2 (FluidSynth via ALSA).
/// Sur macOS  : auto-détection de FluidSynth dans CoreMIDI.
/// Affiche la liste des ports disponibles au démarrage.
///
/// Retourne `None` si aucun port n'est disponible.
/// Retourne le handle ET le nom du port connecté (le nom contient le PID du
/// client ALSA → permet de détecter la disparition du port : FluidSynth
/// redémarré change de nom, la connexion devient muette).
pub fn init_midi() -> Option<(MidiHandle, String)> {
    let mo = MidiOutput::new("chords-server-rs").ok()?;
    let p = mo.ports();
    if p.is_empty() {
        eprintln!("Aucun port MIDI disponible");
        return None;
    }
    println!("Ports MIDI disponibles :");
    for (i, x) in p.iter().enumerate() {
        if let Ok(n) = mo.port_name(x) { println!("  [{}] {}", i, n) }
    }

    // Sélection du port
    let i: usize = if let Ok(e) = std::env::var("MIDI_PORT") {
        // MIDI_PORT défini explicitement → utiliser cet index
        e.parse().unwrap_or(2)
    } else {
        // Pas de MIDI_PORT → auto-détection intelligente
        // Chercher un port FluidSynth ou Roland par nom
        let names: Vec<String> = p.iter().filter_map(|x| mo.port_name(x).ok()).collect();

        // Priorité : FluidSynth (nommé "FLUID" ou "FluidSynth" ou "fluid")
        if let Some((idx, _)) = names.iter().enumerate().find(|(_, n)| {
            n.to_lowercase().contains("fluid")
        }) {
            println!("   → Auto-détection : port FluidSynth [{}] {}", idx, names[idx]);
            idx
        }
        // Priorité 2 : Roland Digital Piano
        else if let Some((idx, _)) = names.iter().enumerate().find(|(_, n)| {
            n.to_lowercase().contains("roland")
        }) {
            println!("   → Auto-détection : port Roland [{}] {}", idx, names[idx]);
            idx
        }
        // Priorité 3 : premier port qui n'est pas "Midi Through" ou "System"
        else if let Some((idx, _)) = names.iter().enumerate().find(|(_, n)| {
            !n.contains("Midi Through") && !n.contains("System")
        }) {
            println!("   → Auto-sélection : [{}] {}", idx, names[idx]);
            idx
        }
        // Fallback : port 2 (comportement Linux historique)
        else {
            2
        }
    };

    if i >= p.len() {
        eprintln!("Port MIDI {} invalide ({} ports disponibles)", i, p.len());
        return None;
    }
    let name = mo.port_name(&p[i]).unwrap_or_default();
    println!("✅ Connecté à MIDI : {}", name);
    mo.connect(&p[i], "chords-server-rs").ok()
        .map(|c| (Arc::new(Mutex::new(c)), name))
}

// ─── Helpers MIDI ──────────────────────────────────────────────────────

/// Envoie un message MIDI brut sur la connexion.
/// Affiche une erreur (non bloquante) si l'envoi échoue.
pub fn snd(c: &mut MidiOutputConnection, m: &[u8]) {
    if let Err(e) = c.send(m) { eprintln!("⚠️ Erreur MIDI : {}", e) }
}

/// Control Change (CC) — message de contrôleur MIDI.
/// Commande : 0xB0 | ch, control, value
pub fn cc(c: &mut MidiOutputConnection, ch: u8, ctl: u8, v: u8) { snd(c, &[0xB0 | ch, ctl, v]) }

/// Program Change — change l'instrument GM sur un canal.
/// Commande : 0xC0 | ch, program
pub fn pc(c: &mut MidiOutputConnection, ch: u8, v: u8) { snd(c, &[0xC0 | ch, v]) }

/// Note On — commence une note.
/// Commande : 0x90 | ch, note, velocity
pub fn no(c: &mut MidiOutputConnection, ch: u8, n: u8, v: u8) { snd(c, &[0x90 | ch, n, v]) }

/// Note On avec scaling par le master volume.
/// La vélocité est multipliée par `mv/127` avant envoi.
pub fn no_mv(c: &mut MidiOutputConnection, ch: u8, n: u8, v: u8, mv: u8) {
    snd(c, &[0x90 | ch, n, ((v as u16 * mv as u16) / 127).min(127) as u8])
}

/// Reset all controllers — envoie CC 123 (All Notes Off) sur chaque canal utilisé.
/// Utilisé entre les accords pour couper les notes qui trainent.
pub fn rch(c: &mut MidiOutputConnection) {
    for &ch in &[0u8, 2, 3, 4, 9] { cc(c, ch, 123, 0) }
}

/// Pitch Bend — change le pitch d'un canal (±2 demi-tons en MIDI standard).
/// Valeur : 0..16383, 8192 = centre (pas de bend).
/// Commande : 0xE0 | ch, lsb, msb
/// Utilisé pour l'accordage 432Hz : 6881 = -0.32 demi-tons = 432Hz au lieu de 440Hz.
pub fn pb(c: &mut MidiOutputConnection, ch: u8, val: u16) {
    let lsb = (val & 127) as u8;
    let msb = ((val >> 7) & 127) as u8;
    snd(c, &[0xE0 | ch, lsb, msb])
}

// ─── Drum hit ──────────────────────────────────────────────────────────

/// Joue les coups de batterie pour un beat donné, selon le pattern sélectionné.
///
/// Appelée à chaque temps (on_beat) et contretemps (on_eighth) pendant la
/// lecture live.  Le pattern détermine quels instruments sont joués à
/// chaque position dans la mesure.
///
/// # Patterns supportés
/// - **rock**    : kick [1,3], snare [2,4], HH sur tous les temps+8èmes
/// - **reggae**  : kick marcato sur beat 2, HH+rimshot, HH légères ailleurs
/// - **jazz**    : ride cymbal principalement, rimshot sur beat 7
/// - **pop**     : kick+snare+HH plus léger que rock
/// - **bossa**   : kick syncopé (1,2,3), snare doux sur beat 1
/// - **onedrop** : kick sur 1 et 3, rimshot sur beat 2, HH variées
///
/// # Paramètres
/// - `beat`    : numéro de beat absolu depuis le début du morceau
/// - `pat`     : identifiant du pattern (PAT_ROCK, PAT_REGGAE, etc.)
/// - `on_beat` : true si c'est un temps fort
/// - `on_eighth` : true si c'est un contretemps en 8ème
/// - `bars`    : nombre de beats par mesure (pour wrap autour de la mesure)
/// - `vol`     : vélocité de base pour les drums (avant scaling)
/// - `mv`      : master volume
fn drum_hit(c: &mut MidiOutputConnection, beat: u64, pat: u8, on_beat: bool, on_eighth: bool, bars: u64, vol: u8, mv: u8) {
    // Ignorer les appels inutiles (ni temps, ni contretemps)
    if !on_beat && !on_eighth { return }

    // Position dans la mesure courante
    let b = beat % bars;

    // Pre-calcul des vélocités scalées par volume (évite les répétitions)
    let v = sc(vol, mv);           // Vélocité drums de base (scalée par master)
    let hh = sc(v, HH_BEAT);      // HH sur le temps
    let h8 = sc(v, HH_8TH);       // HH sur la croche
    let h55 = sc(v, 55);          // HH medium-doux
    let h45 = sc(v, 45);          // HH doux
    let h40 = sc(v, 10);          // HH très doux (ghost note)
    let h60 = sc(v, 60);          // HH medium
    let h65 = sc(v, 65);          // HH medium-fort

    // Sélection du pattern
    match pat {
        PAT_REGGAE => if on_beat {
            // Reggae : kick sur temps 2, HH légères sur 1,3
            match b {
                0 | 1 | 3 => { no(c, 9, DRUM_HH, h60); }
                2 => {
                    no(c, 9, DRUM_KICK, sc(v, 120));
                    no(c, 9, DRUM_HH, h65);
                    no(c, 9, DRUM_RIM, sc(v, 90));
                }
                4 => { no(c, 9, DRUM_HH_OPEN, h55); }
                _ => {}
            }
        } else if on_eighth {
            // Contretemps : HH ghost
            match b {
                0 |2 => {
                    no(c, 9, DRUM_HH, h65);
                }
                _ => {}
            }
        }

        PAT_JAZZ => {
            // Jazz : ride cymbal sur les temps, rimshot sur beat 7
            // Note : b modulo 8 car le jazz joue sur 2 mesures
            let b = beat % 8;
            if on_beat {
                match b {
                    1 | 3 => {
                        no(c, 9, 44, sc(v, 40)); // Hi-Hat pédale (GM 44)
                        no(c, 9, DRUM_RIDE, h60);
                    }
                    _ => { no(c, 9, DRUM_RIDE, h55);}
                }
            } else if on_eighth {
            // Contretemps : HH ghost
            match b {
                3 => {
                    no(c, 9, DRUM_RIDE, h65);
                }
                _ => {}
            }
        }
        }

        PAT_POP => {
            // Pop : kick+snare+HH, un peu plus léger que le rock
            let b = beat % 8;
            if on_beat {
                match b {
                    0 => { no(c, 9, DRUM_KICK, sc(v, 85)); no(c, 9, DRUM_HH, sc(v, 85)); }
                    2 => { no(c, 9, DRUM_SNARE, sc(v, 70)); no(c, 9, DRUM_HH, sc(v, 85)); }
                    3 => { no(c, 9, DRUM_HH, sc(v, 85)); no(c, 9, DRUM_KICK, sc(v, 85));}
                    4 => { no(c, 9, DRUM_KICK, sc(v, 75)); no(c, 9, DRUM_HH, sc(v, 85)); }
                    6 => { no(c, 9, DRUM_SNARE, sc(v, 65)); no(c, 9, DRUM_HH, sc(v, 85)); }
                    _ => { no(c, 9, DRUM_HH, sc(v, 85));}
                }
            } else if on_eighth {
                no(c, 9, DRUM_HH, sc(v, 45));
            }
        }

        PAT_BOSSA => if on_beat {
            // Bossa : kick syncopé, snare très doux
            match b {
                0 => { no(c, 9, DRUM_KICK, sc(v, 55)); no(c, 9, DRUM_HH, h45); }
                1 => { no(c, 9, DRUM_SNARE, sc(v, 30)); no(c, 9, DRUM_HH, h45); }
                2 => { no(c, 9, DRUM_KICK, sc(v, 60)); no(c, 9, DRUM_HH, h45); }
                3 => { no(c, 9, DRUM_KICK, sc(v, 50)); no(c, 9, DRUM_HH, h45); }
                _ => {}
            }
        } else if on_eighth {
            no(c, 9, DRUM_HH, h40);
        }

        PAT_ONEDROP => if on_beat {
            // One-drop reggae : kick sur 1 et 3, rimshot sur 2
            match b {
                0 => { no(c, 9, DRUM_KICK, sc(v, 90)); no(c, 9, DRUM_HH, h55); }
                1 => { no(c, 9, DRUM_HH, h40); }
                2 => { no(c, 9, DRUM_KICK, sc(v, 90)); no(c, 9, DRUM_RIM, sc(v, 65)); no(c, 9, DRUM_HH, h45); }
                3 => { no(c, 9, DRUM_HH, h55); }
                _ => {}
            }
        } 

        // Pattern par défaut : ROCK
        _ => if on_beat {
            // Rock standard : kick [1,3], snare [2,4], HH partout
            match b {
                0 => { no(c, 9, DRUM_KICK, sc(v, 90)); no(c, 9, DRUM_HH, hh); }
                1 => { no(c, 9, DRUM_SNARE, sc(v, 75)); no(c, 9, DRUM_HH, hh); }
                2 => { no(c, 9, DRUM_KICK, sc(v, 80)); no(c, 9, DRUM_HH, hh); }
                3 => { no(c, 9, DRUM_SNARE, sc(v, 70)); no(c, 9, DRUM_HH, hh); }
                _ => {}
            }
        } else if on_eighth {
            no(c, 9, DRUM_HH, h8);
        }
    }
}

// ─── Play notes ─────────────────────────────────────────────────────────

/// Joue une séquence de notes MIDI immédiate (non séquentielle).
/// Utilisé pour les tests rapides ou les déclenchements ponctuels.
///
/// Joue chaque note 2 fois, avec un délai de 240ms entre chaque répétition.
/// Les notes basses sont envoyées sur le canal basse (ch=2), les aiguës sur
/// le canal lead (ch=0).
pub fn play_notes(c: &mut MidiOutputConnection, notes: &[String], mv: u8) {
    let mut v: Vec<u8> = vec![];
    for n in notes { if let Ok(m) = note_midi(n) { v.push(m) } }
    if v.is_empty() { return }

    // Reset + setup RPN (Registered Parameter Number) pour le pitch bend range
    rch(c);
    for &ch in &[0u8, 2, 3] {
        cc(c, ch, 101, 0); cc(c, ch, 100, 1); cc(c, ch, 6, 62); cc(c, ch, 38, 2);
    }
    pc(c, 0, 51); pc(c, 2, 33);

    // Deux répétitions des notes avec 240ms d'intervalle
    for _ in 0..2 {
        for &n in &v {
            std::thread::sleep(Duration::from_millis(240));
            if n < MIN_NOTE {
                // Note grave → canal basse
                no_mv(c, 2, n, 35, mv);
            } else {
                // Note aiguë → canal lead
                no_mv(c, 0, n, 15, mv);
            }
        }
    }
    rch(c);
    println!("  Notes jouées : {:?}", v);
}

// ─── Setup tracks ───────────────────────────────────────────────────────

/// Initialise les pistes en envoyant les Program Change sur chaque canal.
/// Le canal drums (9) utilise le kit par défaut (program 1 = Standard Kit).
/// Les pistes percussion sur d'autres canaux n'ont pas besoin de
/// programmation : leurs notes sont redirigées vers le canal drums natif (9).
/// Applique aussi les sends d'effets live (CC91 reverb, CC93 chorus).
fn setup_tracks(c: &mut MidiOutputConnection, tracks: &[LiveTrack]) {
    for t in tracks {
        let ch = t.channel;
        if t.drums.load(Ordering::Relaxed) && ch != 9 {
            // Piste percussion ajoutée : rien à programmer ici (le canal 9
            // drums natif reçoit ses notes — voir play_seq / note).
            continue;
        } else if ch == 9 {
            // Drums : program 1 = Standard Kit (toujours)
            pc(c, ch, 1);
        } else {
            pc(c, ch, t.program.load(Ordering::Relaxed) as u8);
        }
        // Sends d'effets (FluidSynth les applique en live)
        cc(c, ch, 91, t.fx_reverb.load(Ordering::Relaxed));
        cc(c, ch, 93, t.fx_chorus.load(Ordering::Relaxed));
    }
}

// ─── Apply tracks ───────────────────────────────────────────────────────

/// Applique une configuration externe (depuis le frontend) aux pistes live.
///
/// Règles (pistes dynamiques) :
/// - Canal déjà présent → met à jour program/volume/mute (champs Option).
/// - Canal inconnu → la piste est AJOUTÉE (nouvelle piste instrument).
/// - Canal existant ABSENT de la config → MUTÉ (piste supprimée côté UI :
///   le frontend envoie la liste exacte des pistes actives).
pub fn apply_tracks(lc: &Live, cfg: &[TrackCfg]) {
    let mut tracks = lc.tracks.lock().unwrap();
    for tc in cfg {
        match tracks.iter_mut().find(|t| t.channel == tc.channel) {
            Some(t) => {
                if let Some(p) = tc.program { t.program.store(p, Ordering::Relaxed); }
                if let Some(v) = tc.volume { t.volume.store(v, Ordering::Relaxed); }
                if let Some(m) = tc.mute { t.mute.store(m, Ordering::Relaxed); }
                if let Some(d) = tc.drums { t.drums.store(d, Ordering::Relaxed); }
                if let Some(fx) = tc.effects {
                    t.fx_reverb.store(fx.reverb, Ordering::Relaxed);
                    t.fx_chorus.store(fx.chorus, Ordering::Relaxed);
                }
            }
            None => {
                // Nouvelle piste → l'ajouter (défauts : program 0, vol 100)
                let nt = LiveTrack::new(
                    tc.channel,
                    tc.program.unwrap_or(0),
                    tc.volume.unwrap_or(100),
                );
                nt.drums.store(tc.drums.unwrap_or(false), Ordering::Relaxed);
                if let Some(m) = tc.mute { nt.mute.store(m, Ordering::Relaxed); }
                if let Some(fx) = tc.effects {
                    nt.fx_reverb.store(fx.reverb, Ordering::Relaxed);
                    nt.fx_chorus.store(fx.chorus, Ordering::Relaxed);
                }
                tracks.push(nt);
            }
        }
    }
    // Muter les pistes existantes absentes de la nouvelle config
    let active: Vec<u8> = cfg.iter().map(|tc| tc.channel).collect();
    for t in tracks.iter_mut() {
        if !active.contains(&t.channel) {
            t.mute.store(true, Ordering::Relaxed);
        }
    }
}

// ─── Play sequence (boucle principale live) ────────────────────────────

/// Boucle principale de lecture live d'une séquence d'accords.
///
/// Cette fonction tourne dans un thread séparé et lit la grille d'accords
/// séquentiellement, en générant les notes MIDI en temps réel pour chaque
/// piste (lead, basse, nappes, drums, accent).
///
/// Principe de fonctionnement :
/// 1. Pour chaque accord, on calcule sa durée (en ms) à partir du tempo et
///    du nombre de temps (`beats`).
/// 2. On boucle à l'intérieur de l'accord avec un pas de 1/4 de temps (16ème
///    de note) pour générer les événements rythmiques.
/// 3. À chaque pas, on vérifie :
///    - Si un nouveau temps a commencé → drums on_beat
///    - Si un contretemps est atteint → drums on_eighth
///    - Si la basse doit jouer une nouvelle note (1 par temps)
///    - Si le lead doit jouer la pompe skank (sur contretemps)
///    - Si l'accent 2&4 doit être joué (temps 2 et 4)
/// 4. Si `do_loop` est vrai, la séquence recommence depuis le début
///    jusqu'à réception d'un ordre `stop`.
///
/// La synchronisation se fait via `Instant` + sleep, avec une résolution
/// minimale de 30ms par pas pour éviter de surcharger le CPU.
///
/// # Paramètres
/// - `c`       : connexion MIDI
/// - `ev`      : séquence d'accords (ChordEv)
/// - `lc`      : état live partagé
/// - `do_loop` : si vrai, boucle infinie
pub fn play_seq(c: &mut MidiOutputConnection, ev: &[ChordEv], lc: &Live, do_loop: bool) {
    'outer: loop {
        // Reset : coupe toutes les notes, initialise les programmes
        rch(c);
        // Verrouiller les pistes pour TOUTE la séquence : les références
        // ci-dessous restent valides tant que le guard est vivant. Les
        // handlers HTTP (apply_tracks) attendent la fin de l'itération
        // courante → pas de deadlock.
        let tracks = lc.tracks.lock().unwrap();
        setup_tracks(c, &tracks);
        std::thread::sleep(Duration::from_millis(2));

        // Buffers de notes pour couper les notes précédentes avant
        // de jouer les nouvelles (évite les superpositions)
        let mut prev_nappe: Vec<u8> = vec![];
        let mut prev_lead: Vec<u8> = vec![];
        let mut prev_accent: Vec<u8> = vec![];

        // Rôles par CANAL (lookup dynamique) : une piste absente = muette.
        // Le moteur supporte ainsi l'ajout/suppression de pistes sans
        // hypothèse sur un nombre fixe.
        let t_lead = tracks.iter().find(|t| t.channel == 0);
        let t_bass = tracks.iter().find(|t| t.channel == 2);
        let t_str = tracks.iter().find(|t| t.channel == 3);
        let t_drums = tracks.iter().find(|t| t.channel == 9);
        let t_accent = tracks.iter().find(|t| t.channel == 4);
        let (ch_lead, ch_bass, ch_str, ch_accent) = (0u8, 2u8, 3u8, 4u8);
        let lead_mute = t_lead.map_or(true, |t| t.mute.load(Ordering::Relaxed));
        let bass_mute = t_bass.map_or(true, |t| t.mute.load(Ordering::Relaxed));
        let str_mute = t_str.map_or(true, |t| t.mute.load(Ordering::Relaxed));
        let drums_mute = t_drums.map_or(true, |t| t.mute.load(Ordering::Relaxed));
        let accent_mute = t_accent.map_or(true, |t| t.mute.load(Ordering::Relaxed));
        let vol_lead = t_lead.map(|t| t.volume.load(Ordering::Relaxed)).unwrap_or(0);
        let vol_bass = t_bass.map(|t| t.volume.load(Ordering::Relaxed)).unwrap_or(0);
        let vol_str = t_str.map(|t| t.volume.load(Ordering::Relaxed)).unwrap_or(0);
        let vol_drums = t_drums.map(|t| t.volume.load(Ordering::Relaxed)).unwrap_or(0);
        let vol_accent = t_accent.map(|t| t.volume.load(Ordering::Relaxed)).unwrap_or(0);

        let walking = lc.walking.load(Ordering::Relaxed);
        let mv = lc.master_vol.load(Ordering::Relaxed);

        let mut seed: u64 = 0; // Seed pour la walking bass (déterministe)

        // ── Parcours de la grille d'accords ─────────────────────
        for (i, e) in ev.iter().enumerate() {
            // Convertir les noms de notes textuels en MIDI
            let mut m: Vec<u8> = vec![];
            for n in &e.notes { if let Ok(x) = note_midi(n) { m.push(x) } }

            // ── Cas vide : accord de silence ──────────────────
            // Pas de notes (ex: "4:_") → vrai silence : RIEN ne joue
            // (ni accords, ni nappes, ni drums), mais le timing avance
            // correctement pour que la musique ne soit pas saccadée.
            if m.is_empty() {
                if i > 0 {
                    rch(c);           // Couper les notes qui trainent
                    cc(c, 9, 120, 0);  // All Sound Off sur drums
                }
                let dur = (60_000.0 / lc.tempo.load(Ordering::Relaxed).max(20) as f64 * e.beats) as u64;
                let start = std::time::Instant::now();
                let dur_f = dur as f64;

                // Boucle de silence : attendre sans rien jouer
                while start.elapsed().as_secs_f64() * 1000.0 < dur_f && !lc.stop.load(Ordering::Relaxed) {
                    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                    let remaining = dur_f - elapsed;
                    if remaining > 10.0 {
                        std::thread::sleep(Duration::from_millis(
                            (remaining.min(50.0).max(5.0)) as u64
                        ));
                    }
                }
                if lc.stop.load(Ordering::Relaxed) { break 'outer; }
                continue;
            }

            // ── Début d'un accord non-vide ─────────────────────
            if i > 0 {
                rch(c);           // Couper les notes précédentes
                cc(c, 9, 120, 0);  // All Sound Off drums
            }

            // La première note est la basse (fondamentale)
            let root = m[0];
            // Les notes suivantes sont les chord tones (nappes, lead)
            let nappe_notes: Vec<u8> = m[1..].to_vec();

            // Durée de l'accord en ms
            let dur = (60_000.0 / lc.tempo.load(Ordering::Relaxed).max(20) as f64 * e.beats) as u64;
            let start = std::time::Instant::now();
            let mut idx = 0u64;
            let mut last_b_drums = u64::MAX;
            let mut last_b_bass = 0u64;
            let mut prev_bass_note: u8 = 0;

            // ── Walking bass ────────────────────────────────────
            // Générer 4 notes de walking pour la mesure courante
            let mut walking_notes: [u8; 4] = [root, root, root, root];
            if walking && PAT_REGGAE != lc.pattern.load(Ordering::Relaxed) {
                // Récupérer la fondamentale de l'accord suivant (ou du premier si dernier)
                let next_root = if let Some(ne) = ev.get(i + 1) {
                    let nv = notes_from_vec(&ne.notes);
                    if !nv.is_empty() { nv[0] } else { root }
                } else {
                    if let Some(ne) = ev.get(0) {
                        let nv = notes_from_vec(&ne.notes);
                        if !nv.is_empty() { nv[0] } else { root }
                    } else { root }
                };
                walking_notes = generate_walking_bass(
                    &m,
                    next_root,
                    seed,
                    m.len() >= 2 && is_minor(&m[1..]),
                );
                seed = seed.wrapping_add(1);
            }

            // ── Jouer la première note de basse ─────────────────
            if !bass_mute {
                let bvol = sc(vol_bass, mv);
                let bass_note = if walking { walking_notes[0] } else { root };
                no_mv(c, ch_bass, bass_note, bvol, mv);
                prev_bass_note = bass_note;
                last_b_bass = 0;
            }

            // ── Jouer les nappes ──────────────────────────────
            // En reggae, les nappes ne jouent que sur les accords
            // courts (4:, 8:, 16:) — pas de tenue sur les longs.
            let pat = lc.pattern.load(Ordering::Relaxed);
            let short_chord = e.beats <= 1.0;
            let skip_nappe = pat == PAT_REGGAE && !short_chord;
            if !str_mute && !skip_nappe {
                // Couper les nappes précédentes (Note Off = vélocité 0)
                for n in &prev_nappe { no(c, ch_str, *n, 0); }
                let str_vol = sc(vol_str, mv);
                // Jouer toutes les chord tones en simultané (effet nappe)
                for n in &nappe_notes { no_mv(c, ch_str, *n, str_vol, mv); }
                prev_nappe = nappe_notes.clone();
            }

            // ── Boucle interne : chaque 1/4 de temps ──────────
            let dur_f = dur as f64;
            while start.elapsed().as_secs_f64() * 1000.0 < dur_f && !lc.stop.load(Ordering::Relaxed) {
                let tempo_f = lc.tempo.load(Ordering::Relaxed).max(20) as f64;
                let bd_ms = 60_000.0 / tempo_f;        // Durée d'un temps
                let delay_ms = (bd_ms / 4.0).max(30.0); // Pas = 1/4 de temps
                let target = start + Duration::from_secs_f64(idx as f64 * delay_ms / 1000.0);
                let now = std::time::Instant::now();
                if target > now { std::thread::sleep(target - now); }

                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                let pt = lc.pattern.load(Ordering::Relaxed);
                let sig_val = lc.sig.load(Ordering::Relaxed);
                let bars = (sig_val / 10).max(1) as u64;
                let beat = (elapsed_ms / bd_ms) as u64;

                // ── Drums (sur le temps) ─────────────────
                if !drums_mute {
                    if last_b_drums == u64::MAX || beat > last_b_drums {
                        let dvol = sc(vol_drums, mv);
                        drum_hit(c, beat, pt, true, false, bars, dvol, 127);
                        last_b_drums = beat;
                    }
                    // Drums (sur le contretemps)
                    let beat_pos = elapsed_ms % bd_ms;
                    if beat_pos > bd_ms / 2.0 - 10.0 && beat_pos < bd_ms / 2.0 + 10.0 {
                        let dvol = sc(vol_drums, mv);
                        drum_hit(c, beat, pt, false, true, bars, dvol, 127);
                    }
                }

                // ── Basse (1 note par temps) ────────────
                if !bass_mute {
                    if beat > last_b_bass {
                        let bvol = sc(vol_bass, mv);
                        let bass_note = if walking {
                            let bi = (beat % 4) as usize;
                            walking_notes[bi]
                        } else {
                            root
                        };
                        // Note Off de l'ancienne → Note On de la nouvelle
                        no(c, ch_bass, prev_bass_note, 0);
                        no_mv(c, ch_bass, bass_note, bvol, mv);
                        prev_bass_note = bass_note;
                        last_b_bass = beat;
                    }
                }

                // ── Lead (pompe skank) ──────────────────
                // Staccato sur contretemps : on au 3/4 de temps, off au 1/4 suivant
                if !m.is_empty() {
                    // Note Off des notes précédentes (à idx=3, soit 3/4 du pas)
                    if idx % 4 == 3 && !prev_lead.is_empty() {
                        for &n in &prev_lead { no(c, ch_lead, n, 0); }
                        prev_lead.clear();
                    }
                    // Note On sur contretemps (à idx=2, soit 2/4 = 1/2 pas)
                    if idx % 4 == 2 && !lead_mute {
                        let lvol = sc(vol_lead, mv);
                        prev_lead = m.clone();
                        for &note in &m { no_mv(c, ch_lead, note, lvol, mv); }
                    }
                }

                // ── Accent (temps 2&4) ─────────────────
                // Seulement pour les accords de plus d'1 temps (pas sur
                // les 4:, 8:, 16: qui sont trop courts pour un backbeat).
                if !m.is_empty() && e.beats > 1.0 {
                    // Note Off (à idx=5)
                    if idx % 8 == 5 && !prev_accent.is_empty() {
                        for &n in &prev_accent { no(c, ch_accent, n, 0); }
                        prev_accent.clear();
                    }
                    // Note On sur temps 2 ou 4 (à idx=4)
                    if idx % 8 == 4 && !accent_mute {
                        let avol = sc(vol_accent, mv);
                        prev_accent = m.clone();
                        for &note in &m { no_mv(c, ch_accent, note, avol, mv); }
                    }
                }

                idx += 1;
            }

            // Vérifier le flag stop entre les accords
            if lc.stop.load(Ordering::Relaxed) { break 'outer; }
        }

        // ── Fin de séquence : couper toutes les notes ─────────
        for n in &prev_nappe { no(c, ch_str, *n, 0); }
        for n in &prev_lead { no(c, ch_lead, *n, 0); }
        for n in &prev_accent { no(c, ch_accent, *n, 0); }

        // Si stop demandé ou pas de boucle → sortir
        if lc.stop.load(Ordering::Relaxed) || !do_loop { break; }
    }

    // Nettoyage final : all notes off sur tous les canaux
    rch(c);
    println!("  Lecture terminée ({} accords)", ev.len());
}

// ─── TrackCfg (partagé avec main.rs) ─────────────────────────────────────

/// Configuration d'une piste envoyée depuis le frontend.
/// Utilisée par la route `/config` et `/play` pour modifier
/// les paramètres des pistes en temps réel.
#[derive(Clone, Deserialize)]
pub struct TrackCfg {
    pub channel: u8,                // Canal MIDI cible
    pub program: Option<u16>,       // Nouveau program (None = ne pas changer)
    pub volume: Option<u8>,         // Nouveau volume (None = ne pas changer)
    pub mute: Option<bool>,         // Nouvel état mute (None = ne pas changer)
    /// Piste percussion (kit drums) sur un canal ≠ 9 (None = ne pas changer).
    pub drums: Option<bool>,
    /// Effets (reverb/chorus appliqués en live via CC MIDI 91/93).
    pub effects: Option<crate::dsp::Fx>,
}

// ─── ChordEv (partagé avec main.rs) ──────────────────────────────────────

/// Événement d'accord dans la séquence.
/// Contient les notes (noms textuels) et la durée en temps.
///
/// # Exemple JSON
/// ```json
/// {"notes": ["C4", "E4", "G4"], "beats": 4.0}
/// ```
#[derive(Clone, Deserialize)]
pub struct ChordEv {
    pub notes: Vec<String>,  // Noms de notes MIDI textuels ("C4", "F#3", etc.)
    pub beats: f64,          // Durée en temps (ex: 4.0 = noire, 2.0 = blanche)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live minimal pour les tests (pistes dynamiques).
    fn live_with(tracks: Vec<LiveTrack>) -> Live {
        Live {
            tracks: Mutex::new(tracks),
            pattern: AtomicU8::new(0),
            tempo: AtomicU16::new(120),
            stop: AtomicBool::new(false),
            sig: AtomicU16::new(44),
            walking: AtomicBool::new(false),
            master_vol: AtomicU8::new(127),
            use432: AtomicBool::new(false),
        }
    }

    #[test]
    fn test_apply_tracks_ajoute_et_mute() {
        let lc = live_with(vec![LiveTrack::new(0, 51, 60)]);
        // Config : seule la nouvelle piste canal 5 est active
        apply_tracks(&lc, &[TrackCfg {
            channel: 5, program: Some(80), volume: Some(90), mute: Some(false),
            drums: None,
            effects: None,
        }]);
        let tracks = lc.tracks.lock().unwrap();
        assert_eq!(tracks.len(), 2, "la nouvelle piste doit être ajoutée");
        let t0 = tracks.iter().find(|t| t.channel == 0).unwrap();
        assert!(t0.mute.load(Ordering::Relaxed), "piste absente de la config → mutée");
        let t5 = tracks.iter().find(|t| t.channel == 5).unwrap();
        assert_eq!(t5.program.load(Ordering::Relaxed), 80);
        assert_eq!(t5.volume.load(Ordering::Relaxed), 90);
        assert!(!t5.mute.load(Ordering::Relaxed));
    }

    #[test]
    fn test_apply_tracks_reactive_une_piste_supprimee() {
        let lc = live_with(vec![LiveTrack::new(2, 33, 70), LiveTrack::new(9, 1, 90)]);
        // On réactive le canal 2 avec un nouveau volume
        apply_tracks(&lc, &[TrackCfg {
            channel: 2, program: Some(40), volume: Some(55), mute: Some(false),
            drums: None,
            effects: None,
        }]);
        let tracks = lc.tracks.lock().unwrap();
        let t2 = tracks.iter().find(|t| t.channel == 2).unwrap();
        assert_eq!(t2.program.load(Ordering::Relaxed), 40);
        assert_eq!(t2.volume.load(Ordering::Relaxed), 55);
        assert!(!t2.mute.load(Ordering::Relaxed));
        let t9 = tracks.iter().find(|t| t.channel == 9).unwrap();
        assert!(t9.mute.load(Ordering::Relaxed), "canal 9 absent → muet");
    }

    #[test]
    fn test_apply_tracks_mise_a_jour_sans_duplication() {
        let lc = live_with(vec![LiveTrack::new(0, 51, 60)]);
        apply_tracks(&lc, &[TrackCfg {
            channel: 0, program: Some(10), volume: None, mute: None,
            drums: None,
            effects: None,
        }]);
        let tracks = lc.tracks.lock().unwrap();
        assert_eq!(tracks.len(), 1, "mise à jour sans duplication");
        assert_eq!(tracks[0].program.load(Ordering::Relaxed), 10);
    }
}
