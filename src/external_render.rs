//! Rendu Externe (mode Navig) : joue le morceau sur le périphérique MIDI
//! courant (Roland Digital Piano, expander…) et enregistre sa sortie audio
//! via la carte de capture associée → WAV avec le vrai son du synthétiseur
//! matériel.
//!
//! Contrainte : le rendu est TEMPS RÉEL — le périphérique joue le morceau
//! pendant toute sa durée (le piano est audible). FluidSynth, lui, rend en
//! silence et plus vite que le temps réel ; le Rendu Externe est donc une
//! OPTION de qualité (le vrai moteur de son du matériel), pas un remplaçant.
//!
//! Repli : si aucune carte de capture n'est trouvée pour le périphérique
//! courant → erreur propre (le frontend peut basculer sur FluidSynth).

use std::io::Cursor;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::midi::{self, MidiHandle};
use crate::render::{self, CustomNote, TrackCfg};

/// Événement MIDI programmé (note-on/off) avec temps absolu en ms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimedEv {
    pub time_ms: f64,
    pub on: bool,
    pub channel: u8,
    pub pitch: u8,
    pub velocity: u8,
}

/// Durée totale du morceau en secondes (max des fins de notes).
pub fn total_duration_sec(notes: &[CustomNote], tempo: u32) -> f64 {
    let tempo_ms = 60_000.0 / tempo.max(1) as f64;
    notes
        .iter()
        .map(|n| n.start_time + n.duration)
        .fold(0.0, f64::max)
        * tempo_ms
        / 1000.0
}

/// Programme les événements note-on/off de toutes les notes, triés par temps
/// absolu (ms). Deux événements au même temps gardent l'ordre on → off
/// (l'off d'une note très courte ne coupe pas le on suivant au même instant).
pub fn plan_events(notes: &[CustomNote], tempo: u32) -> Vec<TimedEv> {
    let tempo_ms = 60_000.0 / tempo.max(1) as f64;
    let mut evs: Vec<TimedEv> = Vec::with_capacity(notes.len() * 2);
    for n in notes {
        evs.push(TimedEv {
            time_ms: n.start_time * tempo_ms,
            on: true,
            channel: n.channel,
            pitch: n.pitch,
            velocity: n.velocity.max(1),
        });
        evs.push(TimedEv {
            time_ms: (n.start_time + n.duration) * tempo_ms,
            on: false,
            channel: n.channel,
            pitch: n.pitch,
            velocity: 0,
        });
    }
    evs.sort_by(|a, b| {
        a.time_ms
            .partial_cmp(&b.time_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    evs
}

/// Carte de capture ALSA (id + description issue de /proc/asound/cards).
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureCard {
    pub id: String,
    pub desc: String,
}

/// Parse la sortie de `/proc/asound/cards` :
/// ` 3 [Piano         ]: USB-Audio - Roland Digital Piano`
pub fn parse_cards(text: &str) -> Vec<CaptureCard> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // id avant le '['
        let id = t.split('[').next().map(|s| s.trim().to_string());
        let Some(id) = id else { continue };
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        // description après ']: '
        let desc = t
            .splitn(2, "]: ")
            .nth(1)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if desc.is_empty() {
            continue;
        }
        out.push(CaptureCard { id, desc });
    }
    out
}

/// Liste les cartes de capture ALSA depuis /proc/asound/cards.
pub fn list_capture_cards() -> Vec<CaptureCard> {
    match std::fs::read_to_string("/proc/asound/cards") {
        Ok(text) => parse_cards(&text),
        Err(_) => Vec::new(),
    }
}

/// Mots significatifs d'un nom de port/carte (minuscules, sans la ponctuation,
/// sans les mots génériques ni les nombres) — utilisés pour le matching.
fn significant_words(s: &str) -> std::collections::HashSet<String> {
    let stop = [
        "midi", "port", "through", "synth", "input", "output", "usb", "audio",
        "device", "card", "generic", "hd", "hda", "intel",
    ];
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .filter(|w| !stop.contains(w))
        .map(|w| w.to_string())
        .collect()
}

/// Trouve la carte de capture correspondant à un port MIDI : au moins 2 mots
/// significatifs communs entre le nom du port et la description de la carte
/// (insensible à la casse). Ex. port « Roland Digital Piano:… » ↔ carte
/// « USB-Audio - Roland Digital Piano » → mots {roland, digital, piano}.
/// Retourne le device ALSA (« hw:3,0 ») ou None.
pub fn find_capture_device(midi_port_name: &str, cards: &[CaptureCard]) -> Option<String> {
    let port_words = significant_words(midi_port_name);
    if port_words.is_empty() {
        return None;
    }
    for c in cards {
        let card_words = significant_words(&c.desc);
        let common = port_words.intersection(&card_words).count();
        if common >= 2 {
            return Some(format!("hw:{},0", c.id));
        }
    }
    None
}

/// Coupe le silence au début et à la fin d'un WAV (16 bits, tout format de
/// canaux) : premier/dernier passage au-dessus du seuil, avec une garde de
/// `keep_sec` secondes de chaque côté (pour ne pas tronquer les attaques).
/// WAV illisible ou entièrement silencieux → inchangé.
pub fn trim_silence(wav: &[u8], threshold: f64, keep_sec: f64) -> Vec<u8> {
    use hound::WavReader;
    let Ok(mut reader) = WavReader::new(Cursor::new(wav)) else {
        return wav.to_vec();
    };
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int || spec.sample_rate == 0 {
        return wav.to_vec();
    }
    let samples: Vec<i16> = reader.samples::<i16>().filter_map(|s| s.ok()).collect();
    if samples.is_empty() {
        return wav.to_vec();
    }
    let ch = spec.channels.max(1) as usize;
    let frames = samples.len() / ch;
    if frames < 64 {
        return wav.to_vec();
    }
    let hop = (spec.sample_rate as usize / 100).max(1) * ch; // 10 ms
    let thr_i = (threshold * 32768.0).max(1.0) as i32;

    let mut first: Option<usize> = None;
    let mut last: usize = 0;
    let mut i = 0;
    while i < samples.len() {
        let end = (i + hop).min(samples.len());
        let mut peak = 0i32;
        for &s in &samples[i..end] {
            peak = peak.max((s as i32).abs());
        }
        if peak >= thr_i {
            if first.is_none() {
                first = Some(i);
            }
            last = i;
        }
        i = end;
    }
    let Some(f) = first else {
        return wav.to_vec(); // entièrement silencieux → inchangé
    };
    let keep = (keep_sec * spec.sample_rate as f64).round() as usize * ch;
    let start = f.saturating_sub(keep);
    let end = (last + hop + keep).min(samples.len());

    let mut out = Vec::new();
    if let Ok(mut w) = hound::WavWriter::new(Cursor::new(&mut out), spec) {
        for &s in &samples[start..end] {
            let _ = w.write_sample(s);
        }
        let _ = w.finalize();
    }
    if out.is_empty() {
        wav.to_vec()
    } else {
        out
    }
}

/// Options du rendu externe.
pub struct ExternalOptions {
    pub tempo: u32,
    pub master_vol: u8,
    /// Clic métronome déjà rendu en WAV (FluidSynth) à mixer après capture.
    pub click_wav: Option<Vec<u8>>,
    pub click_gain: f32,
}

/// Joue `notes` sur le périphérique MIDI et enregistre sa sortie audio.
/// Bloque pendant la durée du morceau (+ marges) — c'est le rendu temps réel.
pub fn render_external(
    handle: &MidiHandle,
    notes: &[CustomNote],
    tracks: &[TrackCfg],
    capture: &str,
    tmp_tag: &str,
    opts: &ExternalOptions,
) -> Result<Vec<u8>, String> {
    let duration = total_duration_sec(notes, opts.tempo);
    if duration <= 0.0 {
        return Err("Aucune note à rendre".into());
    }
    let total = duration + 2.0; // marge : démarrage capture + résonance finale
    let raw_path = format!("/tmp/chordj_ext_{}.wav", tmp_tag);
    let wav16_path = format!("/tmp/chordj_ext_{}_16.wav", tmp_tag);
    let _ = std::fs::remove_file(&raw_path);
    let _ = std::fs::remove_file(&wav16_path);

    // 1. Capture audio (S24_3LE : format du Roland ; repli géré par arecord)
    let mut child = Command::new("arecord")
        .args([
            "-D",
            capture,
            "-f",
            "S24_3LE",
            "-r",
            "44100",
            "-c",
            "2",
            "-d",
            &format!("{:.0}", total.ceil()),
            "-t",
            "wav",
            &raw_path,
        ])
        .spawn()
        .map_err(|e| format!("Impossible de lancer arecord : {e}"))?;

    // 2. Setup MIDI : reset + program changes (drums redirigés vers le 9)
    {
        let mut c = handle.lock().unwrap();
        midi::rch(&mut c);
        for t in tracks {
            if t.mute {
                continue;
            }
            let ch = if t.drums && t.channel != 9 { 9 } else { t.channel };
            midi::pc(&mut c, ch, t.program as u8);
            midi::cc(&mut c, ch, 91, t.fx.reverb as u8);
            midi::cc(&mut c, ch, 93, t.fx.chorus as u8);
        }
    }
    // Marge après le reset (le périphérique absorbe le setup)
    std::thread::sleep(Duration::from_millis(800));

    // 3. Jouer les événements (horloge absolue, lock relâché entre envois)
    let evs = plan_events(notes, opts.tempo);
    let start = Instant::now();
    for ev in &evs {
        let target = Duration::from_secs_f64(ev.time_ms / 1000.0);
        while start.elapsed() < target {
            let rest = target - start.elapsed();
            std::thread::sleep(rest.min(Duration::from_millis(5)));
        }
        let mut c = handle.lock().unwrap();
        if ev.on {
            let _ = c.send(&[0x90 | ev.channel, ev.pitch, ev.velocity]);
        } else {
            let _ = c.send(&[0x80 | ev.channel, ev.pitch, 0]);
        }
    }

    // 4. Attendre la fin de la capture (timeout : durée + 10 s)
    let deadline = Instant::now() + Duration::from_secs_f64(total + 10.0);
    loop {
        if let Some(st) = child
            .try_wait()
            .map_err(|e| format!("arecord : {e}"))?
        {
            if !st.success() {
                let _ = std::fs::remove_file(&raw_path);
                return Err("arecord a échoué (format S24_3LE refusé par la carte ?)".into());
            }
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = std::fs::remove_file(&raw_path);
            return Err("Timeout de la capture externe".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // 5. Conversion 24 bits → 16 bits (les helpers WAV du projet sont 16 bits)
    let ok = Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            &raw_path,
            "-ar",
            "44100",
            "-ac",
            "2",
            "-sample_fmt",
            "s16",
            &wav16_path,
        ])
        .status()
        .map_err(|e| format!("ffmpeg : {e}"))?;
    if !ok.success() {
        return Err("Échec de la conversion ffmpeg 24→16 bits".into());
    }
    let wav16 = std::fs::read(&wav16_path).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&raw_path);
    let _ = std::fs::remove_file(&wav16_path);

    // 6. Post-traitement : silence initial coupé, normalisation + master,
    //    clic mixé (mêmes règles que les autres chemins de rendu).
    let trimmed = trim_silence(&wav16, 0.005, 0.05);
    let gained = render::apply_gain(&trimmed, opts.master_vol);
    let final_wav = match &opts.click_wav {
        Some(cw) => render::mix_wavs(&gained, cw, opts.click_gain).unwrap_or(gained),
        None => gained,
    };
    Ok(final_wav)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::{SampleFormat, WavSpec, WavWriter};

    fn note(channel: u8, start: f64, pitch: u8, duration: f64) -> CustomNote {
        CustomNote {
            channel,
            start_time: start,
            pitch,
            duration,
            velocity: 100,
        }
    }

    fn spec16() -> WavSpec {
        WavSpec {
            channels: 2,
            sample_rate: 44100,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        }
    }

    /// Construit un WAV 16 bits stéréo : `silence_s` s de silence, puis
    /// `ton_s` s d'un signal constant `amp`, puis `fin_s` s de silence.
    fn wav_ton(silence_s: f64, ton_s: f64, fin_s: f64, amp: f64) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut w = WavWriter::new(Cursor::new(&mut buf), spec16()).unwrap();
        let sr = 44100usize;
        let v = (amp * 32767.0) as i16;
        let mut write = |sec: f64, val: i16| {
            for _ in 0..(sec * sr as f64) as usize {
                w.write_sample(val).unwrap();
                w.write_sample(val).unwrap();
            }
        };
        write(silence_s, 0);
        write(ton_s, v);
        write(fin_s, 0);
        w.finalize().unwrap();
        buf
    }

    #[test]
    fn duree_totale_prend_le_max_des_fins() {
        assert_eq!(total_duration_sec(&[], 120), 0.0);
        // 4 beats à 120 BPM = 2 s ; note qui finit à 8 beats = 4 s
        let notes = vec![
            note(0, 0.0, 60, 4.0),
            note(0, 4.0, 64, 4.0),
        ];
        assert!((total_duration_sec(&notes, 120) - 4.0).abs() < 1e-9);
        // tempo nul → protégé par max(1) : 8 beats × 60 s = 480 s (pas de panique)
        assert!((total_duration_sec(&notes, 0) - 480.0).abs() < 1e-6);
    }

    #[test]
    fn plan_events_trie_et_calcule_les_temps() {
        let notes = vec![
            note(1, 0.0, 60, 4.0), // on à 0 ms, off à 2000 ms (120 BPM)
            note(2, 2.0, 65, 2.0), // on à 1000 ms, off à 2000 ms
        ];
        let evs = plan_events(&notes, 120);
        assert_eq!(evs.len(), 4);
        // triés par temps
        for w in evs.windows(2) {
            assert!(w[0].time_ms <= w[1].time_ms);
        }
        assert_eq!(evs[0].time_ms, 0.0);
        assert!(evs[0].on && evs[0].pitch == 60 && evs[0].channel == 1);
        assert!((evs[1].time_ms - 1000.0).abs() < 1e-9);
        assert!(evs[1].on && evs[1].pitch == 65 && evs[1].channel == 2);
        assert!((evs[2].time_ms - 2000.0).abs() < 1e-9);
        // même temps : le off (note 60) avant le on (note 65) ? plan_events
        // trie stable → ordre d'insertion : on60, off60, on65, off65…
        // aux temps 2000 on a off60 (inséré avant on65 à 2000) → on d'abord ?
        // Insertion : n1 → on60@0, off60@2000 ; n2 → on65@1000, off65@2000.
        // Tri : 0 (on60), 1000 (on65), 2000 (off60), 2000 (off65). Stable.
        assert!(evs[2].time_ms == 2000.0 && !evs[2].on);
        assert!(evs[3].time_ms == 2000.0 && !evs[3].on);
    }

    #[test]
    fn plan_events_velocite_minimale() {
        let n = CustomNote { channel: 0, start_time: 0.0, pitch: 60, duration: 1.0, velocity: 0 };
        let evs = plan_events(&[n], 120);
        assert_eq!(evs[0].velocity, 1); // jamais 0 au note-on
    }

    #[test]
    fn parse_cards_extrait_id_et_description() {
        let text = " 0 [Generic_1     ]: HDA-Intel - HD-Audio Generic\n 3 [Piano         ]: USB-Audio - Roland Digital Piano\n 5 [USB           ]: USB-Audio - Carte bidon\n";
        let cards = parse_cards(text);
        assert_eq!(cards.len(), 3);
        assert_eq!(cards[0].id, "0");
        assert_eq!(cards[0].desc, "HDA-Intel - HD-Audio Generic");
        assert_eq!(cards[1].id, "3");
        assert_eq!(cards[1].desc, "USB-Audio - Roland Digital Piano");
        // lignes malformées ignorées
        assert_eq!(parse_cards("pas une carte\n\n 1 [X]: desc\n").len(), 1);
    }

    #[test]
    fn find_capture_device_matche_par_nom() {
        let cards = vec![
            CaptureCard { id: "0".into(), desc: "HDA-Intel - HD-Audio Generic".into() },
            CaptureCard { id: "3".into(), desc: "USB-Audio - Roland Digital Piano".into() },
        ];
        // port Roland → carte 3 (insensible à la casse)
        let dev = find_capture_device("Roland Digital Piano:Roland Digital Piano MIDI 1 28:0", &cards);
        assert_eq!(dev.as_deref(), Some("hw:3,0"));
        // port FluidSynth → aucune carte (pas de match)
        assert_eq!(find_capture_device("FLUID Synth (817148):Synth input port (817148:0) 128:0", &cards), None);
        // nom vide → None
        assert_eq!(find_capture_device("", &cards), None);
        // port inconnu → None
        assert_eq!(find_capture_device("Midi Through:Midi Through Port-0 14:0", &cards), None);
        // carte au nom générique : « Generic X » n'a qu'1 mot commun → pas de match
        assert_eq!(find_capture_device("Generic X", &cards), None);
        // port générique sans mots significatifs → None
        assert_eq!(find_capture_device("USB Audio Device", &cards), None);
    }

    #[test]
    fn trim_silence_coupe_le_debut_et_la_fin() {
        // 0,5 s silence + 1 s ton + 0,5 s silence
        let wav = wav_ton(0.5, 1.0, 0.5, 0.25);
        let trimmed = trim_silence(&wav, 0.005, 0.05);
        use hound::WavReader;
        let mut r = WavReader::new(Cursor::new(&trimmed)).unwrap();
        let samples: Vec<i16> = r.samples::<i16>().filter_map(|s| s.ok()).collect();
        let ch = 2usize;
        let frames = samples.len() / ch;
        let sr = 44100usize;
        // durée ≈ 1 s + 2 × 50 ms de garde ≈ 1,1 s (± 10 ms de hop)
        assert!((frames as f64 / sr as f64 - 1.1).abs() < 0.03, "durée={} s", frames as f64 / sr as f64);
        // le premier échantillon doit être du silence (garde de 50 ms)
        assert_eq!(samples[0], 0);
        // le premier échantillon non nul doit arriver vers 50 ms
        let first_nz = samples.iter().position(|&s| s != 0).unwrap() / ch;
        assert!((first_nz as f64 / sr as f64 - 0.05).abs() < 0.02, "premier={} s", first_nz as f64 / sr as f64);
    }

    #[test]
    fn trim_silence_laisse_inchange_un_wav_plein() {
        let wav = wav_ton(0.0, 1.0, 0.0, 0.25);
        let trimmed = trim_silence(&wav, 0.005, 0.05);
        assert_eq!(trimmed.len(), wav.len());
    }

    #[test]
    fn trim_silence_inchange_si_tout_silence_ou_invalide() {
        let silence = wav_ton(1.0, 0.0, 0.0, 0.0);
        assert_eq!(trim_silence(&silence, 0.005, 0.05).len(), silence.len());
        assert_eq!(trim_silence(b"pas un wav", 0.005, 0.05).to_vec(), b"pas un wav");
        assert_eq!(trim_silence(&[], 0.005, 0.05).len(), 0);
    }
}
