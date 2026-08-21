//! engine.rs — Moteurs de rendu offline pour chordZIC.
//!
//! FluidSynth reste le moteur historique (SoundFont GM). Ce module ajoute
//! deux moteurs « instruments libres » 100 % natifs :
//!
//! - **Sfz** : rend les notes via `sfizz_render` (CLI statique de libsfizz —
//!   le même moteur que le plugin VST3 Sfizz). Couvre l'énorme écosystème
//!   SFZ libre (pianos, drums, orchestral…).
//! - **Vst3** : rend les notes via un plugin VST3 chargé par `vst3-host`
//!   (timeline sample-accurate). Couvre Surge XT et tout VST3 natif Linux.
//!
//! Les deux produisent un WAV 44,1 kHz / 16-bit / stéréo, aligné sur le
//! pipeline existant (dsp.rs, mix, clic) : `fit_duration` tronque ou comble
//! de silence pour que le rendu ait EXACTEMENT la durée demandée.

use crate::render::CustomNote;
use midly::num::{u15, u24, u28, u4, u7};
use midly::{Format, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Ticks par noire pour les SMF générés (standard).
pub const TPQ: u16 = 480;

static NEXT_TAG: AtomicU64 = AtomicU64::new(0);
fn next_render_tag() -> String {
    let n = NEXT_TAG.fetch_add(1, Ordering::Relaxed);
    format!("chordj_engine_{}_{}", std::process::id(), n)
}

/// Moteur de rendu sélectionné par l'utilisateur.
#[derive(Debug, Clone, PartialEq)]
pub enum Engine {
    /// FluidSynth + SoundFont (moteur historique, défaut).
    FluidSynth,
    /// Sfizz (libsfizz) avec un fichier SFZ.
    Sfz(String),
    /// Plugin VST3 natif (chemin du bundle .vst3).
    Vst3(String),
    /// FluidSynth avec une SoundFont EXPLICITE (.sf2/.sf3) — par piste.
    Sf2(String),
}

impl Engine {
    /// Nom lisible du moteur (pour les logs/réponses).
    pub fn label(&self) -> &str {
        match self {
            Engine::FluidSynth => "fluidsynth",
            Engine::Sfz(_) => "sfz",
            Engine::Vst3(_) => "vst3",
            Engine::Sf2(_) => "sf2",
        }
    }
}

/// Résout le binaire sfizz_render : PATH d'abord, puis `~/.local/bin`.
pub fn find_sfizz_render() -> Result<String, String> {
    if let Ok(out) = Command::new("sfizz_render").arg("--help").output() {
        if out.status.success() {
            return Ok("sfizz_render".to_string());
        }
    }
    let local = PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".local/bin/sfizz_render");
    if local.exists() {
        return Ok(local.display().to_string());
    }
    Err("sfizz_render introuvable — installe-le dans ~/.local/bin".into())
}

/// Génère un SMF format 0 (midly — encodage standard éprouvé) depuis des
/// notes, avec tempo meta et EndOfTrack. Utilisé par le moteur Sfz.
pub fn smf_from_notes(notes: &[CustomNote], tempo: u32) -> Vec<u8> {
    let mut track: Vec<TrackEvent> = Vec::new();

    // Tempo : µs par noire (60 000 000 / BPM), clampé ≥ 1 BPM.
    let tempo_us = (60_000_000u64 / tempo.max(1) as u64) as u32;
    track.push(TrackEvent {
        delta: 0.into(),
        kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::from(tempo_us))),
    });

    // Événements on/off triés (on avant off au même tick).
    let mut events: Vec<(u32, u8, u8)> = Vec::new(); // (tick, note, vel ou 0=off)
    for n in notes {
        if n.duration <= 0.0 {
            continue;
        }
        let start = (n.start_time * TPQ as f64).round().max(0.0) as u32;
        let end = ((n.start_time + n.duration) * TPQ as f64).round().max(0.0) as u32;
        if end <= start {
            continue;
        }
        let vel = n.velocity.clamp(1, 127);
        events.push((start, n.pitch, vel));
        events.push((end, n.pitch, 0));
    }
    events.sort_by_key(|(t, n, v)| (*t, if *v == 0 { 1 } else { 0 }, *n));

    let mut prev = 0u32;
    for (t, note, v) in events {
        let ch = u4::from(note.min(15));
        let kind = if v > 0 {
            TrackEventKind::Midi {
                channel: ch,
                message: MidiMessage::NoteOn {
                    key: u7::from(note),
                    vel: u7::from(v),
                },
            }
        } else {
            TrackEventKind::Midi {
                channel: ch,
                message: MidiMessage::NoteOff {
                    key: u7::from(note),
                    vel: u7::from(0),
                },
            }
        };
        track.push(TrackEvent {
            delta: u28::from(t - prev),
            kind,
        });
        prev = t;
    }

    track.push(TrackEvent {
        delta: 0.into(),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });

    let smf = Smf {
        header: midly::Header {
            format: Format::SingleTrack,
            timing: Timing::Metrical(u15::from(TPQ)),
        },
        tracks: vec![track],
    };
    let mut buf = Vec::new();
    if smf.write_std(&mut buf).is_err() {
        return Vec::new();
    }
    buf
}

/// Ajuste un WAV PCM i16 à la durée exacte demandée : tronque s'il est plus
/// long, comble de silence s'il est plus court (le rendu SFZ/VST3 peut être
/// plus court que la durée du morceau — le `--use-eot` s'arrête au dernier
/// événement). La spec du WAV est conservée.
pub fn fit_duration(wav: &[u8], duration_sec: f64) -> Vec<u8> {
    use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
    let Ok(mut reader) = WavReader::new(std::io::Cursor::new(wav)) else {
        return wav.to_vec();
    };
    let spec = reader.spec();
    let ch = spec.channels.max(1) as usize;
    let samples: Vec<i16> = reader.samples::<i16>().filter_map(|s| s.ok()).collect();
    let frames = samples.len() / ch;

    let expected_frames = (duration_sec * spec.sample_rate as f64).round() as usize;
    if frames == expected_frames {
        return wav.to_vec();
    }

    let out_spec = WavSpec {
        channels: spec.channels,
        sample_rate: spec.sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut out = Vec::new();
    if let Ok(mut w) = WavWriter::new(std::io::Cursor::new(&mut out), out_spec) {
        if frames >= expected_frames {
            for f in 0..expected_frames {
                for c in 0..ch {
                    let _ = w.write_sample(samples[f * ch + c]);
                }
            }
        } else {
            for f in 0..frames {
                for c in 0..ch {
                    let _ = w.write_sample(samples[f * ch + c]);
                }
            }
            for _ in frames..expected_frames {
                for _ in 0..ch {
                    let _ = w.write_sample(0i16);
                }
            }
        }
        let _ = w.finalize();
    }
    if out.is_empty() {
        wav.to_vec()
    } else {
        out
    }
}

/// Rendu SFZ : notes → SMF (midly) → `sfizz_render` → WAV 44,1 kHz.
pub fn render_sfz(
    notes: &[CustomNote],
    tempo: u32,
    sfz_path: &str,
    duration_sec: f64,
) -> Result<Vec<u8>, String> {
    let bin = find_sfizz_render()?;
    let smf = smf_from_notes(notes, tempo);
    if smf.is_empty() {
        return Err("Génération du SMF impossible".into());
    }

    let tag = next_render_tag();
    let mid_path = std::env::temp_dir().join(format!("{tag}.mid"));
    let wav_path = std::env::temp_dir().join(format!("{tag}.wav"));
    std::fs::write(&mid_path, &smf)
        .map_err(|e| format!("Impossible d'écrire le MIDI temporaire : {e}"))?;

    // --use-eot : s'arrête au dernier événement (pas de rendu infini) ;
    // -s 44100 : aligné sur le pipeline existant (dsp/mix/clic).
    let output = Command::new(&bin)
        .arg("--use-eot")
        .arg("-s").arg("44100")
        .arg("--sfz").arg(sfz_path)
        .arg("--midi").arg(&mid_path)
        .arg("--wav").arg(&wav_path)
        .output()
        .map_err(|e| format!("Impossible d'exécuter sfizz_render : {e}"))?;

    let _ = std::fs::remove_file(&mid_path);

    if !output.status.success() {
        let _ = std::fs::remove_file(&wav_path);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("sfizz_render a échoué : {}", stderr.trim()));
    }

    let wav = std::fs::read(&wav_path)
        .map_err(|e| format!("Impossible de lire le WAV sfizz : {e}"))?;
    let _ = std::fs::remove_file(&wav_path);

    // Lecture en f32 (sfizz écrit du i16 ou du f32 selon la version), puis
    // normalisation RMS bornée par la crête (jamais de clipping — la
    // dynamique du piano/des percussions est préservée), réécriture i16,
    // ajustement de la durée exacte.
    let (left, right) = read_stereo_f32(&wav)?;
    let (left, right) = normalize_channels(&left, &right, -24.0);
    let wav16 = write_i16_wav_stereo(&left, &right, 44_100)?;

    Ok(fit_duration(&wav16, duration_sec))
}

/// Rendu VST3 : notes → événements datés (frames) → plugin → WAV.
///
/// **Isolé dans un sous-processus** (`--render-vst3-worker`, même binaire en
/// mode worker) : le plugin Surge XT est chargé DANS LE WORKER, jamais dans
/// le process serveur. C'est indispensable depuis que le moteur live charge
/// AUSSI Surge XT in-process : deux instances du plugin dans le même process
/// (moteur live temps réel + rendu offline) déstabilisaient le moteur live
/// (plus de son du monitoring pendant les rendus VST3). Le worker isole
/// aussi le serveur des crashs/chargements lourds du plugin.
///
/// Le chemin peut être un bundle `.vst3` (rendu avec le patch du plugin)
/// OU un preset Surge `.fxp` (le plugin Surge XT est chargé puis le state
/// XML du preset est appliqué — `load_preset` refuse le .fxp Surge, il faut
/// extraire le XML depuis `<?xml` et passer par `load_state`).
pub fn render_vst3(
    notes: &[CustomNote],
    tempo: u32,
    plugin_path: &str,
    duration_sec: f64,
) -> Result<Vec<u8>, String> {
    // Notes sérialisées en JSON compact ([[canal,start,pitch,dur,vel],…])
    let mut json = String::from("[");
    for (i, n) in notes.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&format!(
            "[{},{},{},{},{}]",
            n.channel, n.start_time, n.pitch, n.duration, n.velocity
        ));
    }
    json.push(']');

    let tag = next_render_tag();
    let json_path = std::env::temp_dir().join(format!("{tag}.json"));
    let wav_path = std::env::temp_dir().join(format!("{tag}.wav"));
    std::fs::write(&json_path, json)
        .map_err(|e| format!("Écriture des notes temporaires impossible : {e}"))?;

    let exe = std::env::current_exe()
        .map_err(|e| format!("current_exe : {e}"))?;
    let output = std::process::Command::new(&exe)
        .arg("--render-vst3-worker")
        .arg(&json_path)
        .arg(tempo.to_string())
        .arg(plugin_path)
        .arg(&wav_path)
        .arg(duration_sec.to_string())
        .output()
        .map_err(|e| format!("Lancement du worker VST3 impossible : {e}"))?;
    let _ = std::fs::remove_file(&json_path);

    if !output.status.success() {
        let _ = std::fs::remove_file(&wav_path);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Rendu VST3 (worker) : {}", stderr.trim()));
    }
    let wav = std::fs::read(&wav_path)
        .map_err(|e| format!("Lecture du WAV VST3 impossible : {e}"))?;
    let _ = std::fs::remove_file(&wav_path);

    Ok(fit_duration(&wav, duration_sec))
}

/// Corps du rendu VST3 offline — exécuté dans le SOUS-PROCESSUS worker
/// (`--render-vst3-worker`) : chargement du plugin, application du preset,
/// rendu sample-accurate, normalisation. Le process serveur ne charge
/// jamais le plugin (conflit avec le moteur live).
///
/// Scheduling maison (la 0.4.2 publiée n'a pas `transport::Timeline`) :
/// chaque événement est envoyé avec son offset sample exact dans le block,
/// via `Plugin::send_midi_event_at`.
pub fn render_vst3_offline(
    notes: &[CustomNote],
    tempo: u32,
    plugin_path: &str,
    duration_sec: f64,
) -> Result<Vec<u8>, String> {
    use vst3_host::audio::AudioBuffers;
    use vst3_host::midi::{MidiChannel, MidiEvent};
    use vst3_host::Vst3Host;

    let sample_rate = 44_100.0f64;
    let block = 512usize;

    // Chemin .fxp → preset Surge : le plugin réel est Surge XT, le .fxp
    // n'est que le state à appliquer.
    let is_preset = plugin_path.to_lowercase().ends_with(".fxp");
    let actual_plugin = if is_preset {
        crate::live_instrument::find_surge_plugin()?
    } else {
        plugin_path.to_string()
    };

    let mut host = Vst3Host::builder()
        .sample_rate(sample_rate)
        .block_size(block)
        .build()
        .map_err(|e| format!("Création de l'hôte VST3 impossible : {e}"))?;
    let mut plugin = host
        .load_plugin(&actual_plugin)
        .map_err(|e| format!("Chargement du plugin VST3 impossible : {e}"))?;

    if is_preset {
        let data = std::fs::read(plugin_path)
            .map_err(|e| format!("Lecture du preset impossible : {e}"))?;
        let xml = crate::live_instrument::extract_xml_state(&data)
            .ok_or_else(|| format!("Pas de XML trouvé dans {plugin_path}"))?;
        plugin
            .load_state(xml)
            .map_err(|e| format!("Application du preset impossible : {e}"))?;
    }

    let out_channels = plugin.output_channel_count().max(1);
    let total_frames = (duration_sec * sample_rate).round() as usize;

    // Événements datés en frames absolus (beat → frame), triés.
    let spb = sample_rate * 60.0 / tempo.max(1) as f64; // samples per beat
    let mut events_by_frame: Vec<(u64, MidiEvent)> = Vec::new();
    for n in notes {
        if n.duration <= 0.0 {
            continue;
        }
        let ch = MidiChannel::from_index(n.channel.min(15))
            .unwrap_or(MidiChannel::Ch1);
        let on_frame = (n.start_time * spb).round().max(0.0) as u64;
        let off_frame = ((n.start_time + n.duration) * spb).round().max(0.0) as u64;
        if off_frame <= on_frame {
            continue;
        }
        events_by_frame.push((
            on_frame,
            MidiEvent::NoteOn {
                channel: ch,
                note: n.pitch,
                velocity: n.velocity.clamp(1, 127),
            },
        ));
        events_by_frame.push((
            off_frame,
            MidiEvent::NoteOff {
                channel: ch,
                note: n.pitch,
                velocity: 0,
            },
        ));
    }
    events_by_frame.sort_by_key(|(f, _)| *f);

    // Rendu block par block (freewheeling).
    plugin
        .start_processing()
        .map_err(|e| format!("start_processing : {e}"))?;
    let mut channels: Vec<Vec<f32>> = vec![Vec::with_capacity(total_frames); out_channels];
    let mut clock: u64 = 0;
    let mut idx = 0usize;
    let mut rendered = 0usize;
    while rendered < total_frames {
        let frames = block.min(total_frames - rendered);
        let end = clock + frames as u64;
        while idx < events_by_frame.len() && events_by_frame[idx].0 < end {
            let (frame, ev) = events_by_frame[idx].clone();
            plugin
                .send_midi_event_at(ev, (frame - clock) as i32)
                .map_err(|e| format!("send_midi_event_at : {e}"))?;
            idx += 1;
        }
        let mut buffers = AudioBuffers::new(0, out_channels, frames, sample_rate);
        plugin
            .process_audio(&mut buffers)
            .map_err(|e| format!("process_audio : {e}"))?;
        for (ch_idx, dst) in channels.iter_mut().enumerate() {
            if let Some(src) = buffers.outputs.get(ch_idx) {
                dst.extend_from_slice(&src[..frames.min(src.len())]);
            }
        }
        clock = end;
        rendered += frames;
    }
    plugin
        .stop_processing()
        .map_err(|e| format!("stop_processing : {e}"))?;

    // Stéréo i16 (mix des canaux du plugin — Surge sort 6 canaux, on prend L/R),
    // normalisée RMS bornée par la crête (comme les banques SFZ : niveau perçu
    // comparable, dynamique préservée, jamais de clipping).
    let left = channels.first().cloned().unwrap_or_default();
    let right = channels.get(1).cloned().unwrap_or_else(|| left.clone());
    let (left, right) = normalize_channels(&left, &right, -24.0);
    let buf = write_i16_wav_stereo(&left, &right, sample_rate as u32)?;
    Ok(fit_duration(&buf, duration_sec))
}

/// Rendu complet par piste avec le moteur choisi, puis mix + normalisation
/// (même logique que `render::render_wav_mixed` avec FluidSynth).
pub fn render_wav_with_engine(
    all_notes: &[CustomNote],
    cfg: &crate::render::RenderCfg,
    engine_by_channel: &std::collections::HashMap<u8, Engine>,
    duration_sec: f64,
    master_vol: u8,
    soundfont: &str,
) -> Result<Vec<u8>, String> {
    use crate::render::RenderedTrack;

    let delay_ms = (30_000u32 / cfg.tempo.max(20)).max(1);
    let tempo = cfg.tempo.max(20) as u32;

    // Pistes actives (non mutées) + drums par défaut si besoin (même logique
    // que render_tracks_list — dupliquée ici pour rester autonome).
    let mut tracks: Vec<crate::render::TrackCfg> =
        cfg.tracks.iter().filter(|t| !t.mute).cloned().collect();
    let has_ch9 = tracks.iter().any(|t| t.channel == 9);
    if !has_ch9 && all_notes.iter().any(|n| n.channel == 9) {
        tracks.push(crate::render::TrackCfg {
            channel: 9,
            program: 1,
            volume: 127,
            mute: false,
            drums: false,
            bank_msb: 0,
            bank_lsb: 0,
            fx: Default::default(),
        });
    }

    let mut rendered: Vec<RenderedTrack> = Vec::new();
    for t in &tracks {
        let notes: Vec<CustomNote> = all_notes
            .iter()
            .filter(|n| n.channel == t.channel)
            .cloned()
            .collect();
        if notes.is_empty() {
            continue;
        }
        let engine = engine_by_channel
            .get(&t.channel)
            .cloned()
            .unwrap_or(Engine::FluidSynth); // piste sans instrument → FluidSynth
        let wav = match &engine {
            Engine::FluidSynth => {
                let smf = crate::render::generate_smf_from_custom(&notes, std::slice::from_ref(t), tempo as u16);
                crate::render::render_wav_raw(&smf, soundfont, duration_sec)?
            }
            Engine::Sfz(p) => render_sfz(&notes, tempo, p.as_str(), duration_sec)?,
            Engine::Vst3(p) => render_vst3(&notes, tempo, p.as_str(), duration_sec)?,
            Engine::Sf2(p) => {
                let smf = crate::render::generate_smf_from_custom(&notes, std::slice::from_ref(t), tempo as u16);
                crate::render::render_wav_raw(&smf, p.as_str(), duration_sec)?
            }
        };
        // Les moteurs Sfz/VST3 normalisent eux-mêmes leur sortie (RMS bornée
        // par la crête — voir normalize_channels) ; FluidSynth/Sf2 sortent
        // déjà équilibrés. Le mix final re-normalise au pic (mix_tracks).
        let wav = if t.fx.is_off() {
            wav
        } else {
            crate::dsp::apply_fx(&wav, &t.fx, delay_ms)
        };
        rendered.push(RenderedTrack {
            channel: t.channel,
            wav,
        });
    }

    if rendered.is_empty() {
        return Err("Aucune piste à rendre".into());
    }
    mix_tracks(&rendered, master_vol)
}

/// Mixe des pistes rendues (somme, normalisation au pic ~0,5, gain master,
/// fade-out final) — factorisé depuis `render::render_wav_mixed`.
pub fn mix_tracks(tracks: &[crate::render::RenderedTrack], master_vol: u8) -> Result<Vec<u8>, String> {
    use hound::{SampleFormat, WavReader, WavSpec, WavWriter};

    let mut mix_l: Option<Vec<f32>> = None;
    let mut mix_r: Option<Vec<f32>> = None;

    for rt in tracks {
        let Ok(mut rd) = WavReader::new(std::io::Cursor::new(&rt.wav)) else {
            continue;
        };
        let ch = rd.spec().channels as usize;
        let s: Vec<i16> = rd.samples::<i16>().filter_map(|x| x.ok()).collect();
        let n = s.len() / ch.max(1);
        if n == 0 {
            continue;
        }
        if mix_l.is_none() {
            mix_l = Some(vec![0f32; n]);
            mix_r = Some(vec![0f32; n]);
        }
        if let (Some(ml), Some(mr)) = (mix_l.as_mut(), mix_r.as_mut()) {
            let m = ml.len().min(n);
            for i in 0..m {
                ml[i] += s[i * ch] as f32 / 32768.0;
                mr[i] += if ch > 1 {
                    s[i * ch + 1] as f32 / 32768.0
                } else {
                    s[i * ch] as f32 / 32768.0
                };
            }
        }
    }

    let (Some(ml), Some(mr)) = (mix_l, mix_r) else {
        return Err("Aucune piste à mixer".into());
    };

    let mut pic = 0f32;
    for i in 0..ml.len() {
        pic = pic.max(ml[i].abs()).max(mr[i].abs());
    }
    // Normalisation au pic ~0.8 (marge −1,9 dBFS) : niveau comparable à
    // FluidSynth avec du headroom — les attaques ne saturent pas les
    // enceintes (les pistes individuelles sont déjà bornées à 0,85).
    let norm = if pic > 1e-6 { 0.8 / pic } else { 1.0 };
    let gain = norm * (master_vol as f32) / 127.0;

    let spec = WavSpec {
        channels: 2,
        sample_rate: 44100,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut buf = Vec::new();
    if let Ok(mut w) = WavWriter::new(std::io::Cursor::new(&mut buf), spec) {
        for i in 0..ml.len() {
            let _ = w.write_sample(((ml[i] * gain).clamp(-1.0, 1.0) * 32767.0) as i16);
            let _ = w.write_sample(((mr[i] * gain).clamp(-1.0, 1.0) * 32767.0) as i16);
        }
        let _ = w.finalize();
    }
    if buf.is_empty() {
        return Err("Écriture du WAV impossible".into());
    }
    Ok(crate::render::fade_out_wav(&buf, 30))
}

/// Lit un WAV (Int 8/16/24/32 ou Float, mono ou multi-canaux) en stéréo f32 :
/// L = canal 1, R = canal 2 (mono → dupliqué, canaux supplémentaires ignorés).
pub fn read_stereo_f32(wav: &[u8]) -> Result<(Vec<f32>, Vec<f32>), String> {
    use hound::{SampleFormat, WavReader};
    let mut reader = WavReader::new(std::io::Cursor::new(wav)).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let ch = spec.channels.max(1) as usize;
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut idx = 0usize;
    match spec.sample_format {
        SampleFormat::Float => {
            for s in reader.samples::<f32>() {
                let s = s.map_err(|e| e.to_string())?;
                if idx % ch == 0 { left.push(s); }
                else if idx % ch == 1 { right.push(s); }
                idx += 1;
            }
        }
        SampleFormat::Int => match spec.bits_per_sample {
            8 => {
                for s in reader.samples::<i8>() {
                    let s = s.map_err(|e| e.to_string())? as f32 / 128.0;
                    if idx % ch == 0 { left.push(s); }
                    else if idx % ch == 1 { right.push(s); }
                    idx += 1;
                }
            }
            16 => {
                for s in reader.samples::<i16>() {
                    let s = s.map_err(|e| e.to_string())? as f32 / 32768.0;
                    if idx % ch == 0 { left.push(s); }
                    else if idx % ch == 1 { right.push(s); }
                    idx += 1;
                }
            }
            24 => {
                for s in reader.samples::<i32>() {
                    let s = s.map_err(|e| e.to_string())? as f32 / 8_388_608.0;
                    if idx % ch == 0 { left.push(s); }
                    else if idx % ch == 1 { right.push(s); }
                    idx += 1;
                }
            }
            32 => {
                for s in reader.samples::<i32>() {
                    let s = s.map_err(|e| e.to_string())? as f32 / 2_147_483_648.0;
                    if idx % ch == 0 { left.push(s); }
                    else if idx % ch == 1 { right.push(s); }
                    idx += 1;
                }
            }
            b => return Err(format!("bits par échantillon non gérés : {b}")),
        },
    }
    if ch == 1 {
        // Mono : R = copie de L
        let r = left.clone();
        return Ok((left, r));
    }
    Ok((left, right))
}

/// Normalise des canaux stéréo f32 à un niveau RMS cible (dBFS, négatif), en
/// BORNANT le gain par la crête (headroom 0,9) : les transients forts (piano,
/// percussions) ne sont JAMAIS poussés dans un soft clip — la dynamique est
/// préservée, pas de distorsion. Les instruments doux (cymbales échantillonnées
/// en douceur) reçoivent le gain RMS complet → balance prévisible.
pub fn normalize_channels(left: &[f32], right: &[f32], target_db: f32) -> (Vec<f32>, Vec<f32>) {
    let n = left.len().min(right.len());
    if n == 0 {
        return (left.to_vec(), right.to_vec());
    }
    let mut sum_sq = 0f64;
    let mut peak = 0f32;
    for i in 0..n {
        let l = left[i] as f64;
        let r = right[i] as f64;
        sum_sq += l * l + r * r;
        peak = peak.max(left[i].abs()).max(right[i].abs());
    }
    let rms = (sum_sq / (2.0 * n as f64)).sqrt() as f32;
    if rms < 1e-6 || peak < 1e-6 {
        return (left.to_vec(), right.to_vec());
    }
    let target = 10f32.powf(target_db / 20.0);
    let gain_rms = target / rms;
    // Limite de crête : jamais plus de 0,85 (≈ −1,4 dBFS) après gain —
    // les attaques (piano, percussions) gardent de la marge pour le mix.
    let gain = gain_rms.min(0.85 / peak);
    let mut l = Vec::with_capacity(n);
    let mut r = Vec::with_capacity(n);
    for i in 0..n {
        l.push(left[i] * gain);
        r.push(right[i] * gain);
    }
    (l, r)
}

/// Écrit un WAV stéréo i16 depuis des canaux f32 (clamp à ±1 — jamais de wrap).
pub fn write_i16_wav_stereo(left: &[f32], right: &[f32], sample_rate: u32) -> Result<Vec<u8>, String> {
    use hound::{SampleFormat, WavSpec, WavWriter};
    let spec = WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut buf = Vec::new();
    if let Ok(mut w) = WavWriter::new(std::io::Cursor::new(&mut buf), spec) {
        let n = left.len().min(right.len());
        for i in 0..n {
            let l = (left[i].clamp(-1.0, 1.0) * 32767.0) as i16;
            let r = (right[i].clamp(-1.0, 1.0) * 32767.0) as i16;
            let _ = w.write_sample(l);
            let _ = w.write_sample(r);
        }
        let _ = w.finalize();
    }
    if buf.is_empty() {
        return Err("Écriture du WAV impossible".into());
    }
    Ok(buf)
}

/// Scan récursif des fichiers `.sfz` dans un dossier d'instruments.
/// Retourne (nom lisible, chemin absolu).
pub fn scan_sfz_instruments(root: &std::path::Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Some(ext) = p.extension() {
                    if ext.eq_ignore_ascii_case("sfz") {
                        if let Some(name) = p.file_stem() {
                            out.push((name.to_string_lossy().into_owned(), p.display().to_string()));
                        }
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La normalisation RMS bornée par la crête : les transients chauds
    /// (piano, percussions) ne sont JAMAIS poussés dans un soft clip — le
    /// gain est limité pour que le pic reste ≤ 0,9, dynamique préservée.
    #[test]
    fn normalisation_bornée_par_la_crête() {
        // Attaque très chaude (pic 0,95) + corps doux (0,05)
        let mut l = vec![0.05f32; 44_100];
        l[0] = 0.95;
        let r = l.clone();
        let (nl, nr) = normalize_channels(&l, &r, -24.0);
        let peak = nl.iter().chain(nr.iter()).fold(0f32, |a, &s| a.max(s.abs()));
        assert!(peak <= 0.85 + 1e-6, "pic {peak} > 0,85 — clipping !");
        // L'attaque est préservée (le gain est borné, pas de tanh)
        assert!((nl[0] - 0.85).abs() < 0.02, "attaque écrasée : {}", nl[0]);
        // Le gain n'est pas nul pour autant : le corps a bien été amplifié
        assert!(nl[100] > 0.04, "corps non amplifié : {}", nl[100]);
    }

    /// Instrument doux et uniforme : le gain RMS complet est appliqué
    /// (cible −24 dBFS ≈ 0,063 RMS), la balance reste prévisible.
    #[test]
    fn normalisation_rms_applique_la_cible() {
        let quiet = vec![0.01f32; 1000];
        let (ql, qr) = normalize_channels(&quiet, &quiet, -24.0);
        let rms = (ql.iter().zip(qr.iter()).map(|(a, b)| a * a + b * b).sum::<f32>()
            / (2.0 * ql.len() as f32)).sqrt();
        assert!((rms - 0.0631).abs() < 0.004, "RMS {rms} ≠ cible 0,063");
    }

    /// Lecture stéréo : mono dupliqué, i16 converti proprement en f32.
    #[test]
    fn lecture_stereo_depuis_i16_mono() {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut buf = Vec::new();
        {
            let mut w = hound::WavWriter::new(std::io::Cursor::new(&mut buf), spec).unwrap();
            w.write_sample(16_384i16).unwrap();
            w.write_sample(-16_384i16).unwrap();
            w.finalize().unwrap();
        }
        let (l, r) = read_stereo_f32(&buf).unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(r.len(), 2);
        assert!((l[0] - 0.5).abs() < 1e-4);
        assert!((r[0] - l[0]).abs() < 1e-9); // mono → R = L
        assert!((l[1] + 0.5).abs() < 1e-4);
    }
}
