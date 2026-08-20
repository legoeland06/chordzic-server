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
}

impl Engine {
    /// Nom lisible du moteur (pour les logs/réponses).
    pub fn label(&self) -> &str {
        match self {
            Engine::FluidSynth => "fluidsynth",
            Engine::Sfz(_) => "sfz",
            Engine::Vst3(_) => "vst3",
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

    Ok(fit_duration(&wav, duration_sec))
}

/// Rendu VST3 : notes → événements datés (frames) → plugin → WAV.
///
/// Scheduling maison (la 0.4.2 publiée n'a pas `transport::Timeline`) :
/// chaque événement est envoyé avec son offset sample exact dans le block,
/// via `Plugin::send_midi_event_at`.
pub fn render_vst3(
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

    let mut host = Vst3Host::builder()
        .sample_rate(sample_rate)
        .block_size(block)
        .build()
        .map_err(|e| format!("Création de l'hôte VST3 impossible : {e}"))?;
    let mut plugin = host
        .load_plugin(plugin_path)
        .map_err(|e| format!("Chargement du plugin VST3 impossible : {e}"))?;

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

    // Stéréo i16 (mix des canaux du plugin — Surge sort 6 canaux, on prend L/R).
    let left: Vec<f32> = channels[0].clone();
    let right: Vec<f32> = channels
        .get(1)
        .cloned()
        .unwrap_or_else(|| left.clone());
    use hound::{SampleFormat, WavSpec, WavWriter};
    let spec = WavSpec {
        channels: 2,
        sample_rate: sample_rate as u32,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut buf = Vec::new();
    if let Ok(mut w) = WavWriter::new(std::io::Cursor::new(&mut buf), spec) {
        for i in 0..left.len().min(right.len()) {
            let l = (left[i].clamp(-1.0, 1.0) * 32767.0) as i16;
            let r = (right[i].clamp(-1.0, 1.0) * 32767.0) as i16;
            let _ = w.write_sample(l);
            let _ = w.write_sample(r);
        }
        let _ = w.finalize();
    }
    if buf.is_empty() {
        return Err("Écriture du WAV VST3 impossible".into());
    }
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
        };
        // Les banques SFZ (ex. piano Salamander) et les patches VST3 sont
        // souvent échantillonnés en douceur — bien plus faibles que les
        // instruments GM de FluidSynth. Gain fixe de compensation (+6 dB)
        // sur les pistes SFZ/VST3, pour un niveau de présence comparable.
        // (Pas de normalisation par piste : elle s'additionnerait au pic du
        // mix et ferait tout redescendre à la normalisation finale.)
        let wav = match &engine {
            Engine::FluidSynth => wav,
            Engine::Sfz(_) | Engine::Vst3(_) => apply_gain_linear(&wav, 2.0),
        };
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
    // Normalisation au pic ~0.9 (au lieu de 0.5) : le rendu instruments
    // libres sort avec un niveau comparable à FluidSynth, pas en dessous.
    let norm = if pic > 1e-6 { 0.9 / pic } else { 1.0 };
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

/// Applique un gain linéaire uniforme à un WAV PCM i16 (avec saturation
/// douce tanh au-delà de 1.0 — pas de clipping dur). Spec conservée.
pub fn apply_gain_linear(wav: &[u8], gain: f32) -> Vec<u8> {
    use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
    let Ok(mut reader) = WavReader::new(std::io::Cursor::new(wav)) else {
        return wav.to_vec();
    };
    let spec = reader.spec();
    let samples: Vec<i16> = reader.samples::<i16>().filter_map(|s| s.ok()).collect();
    if samples.is_empty() {
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
        for &s in &samples {
            let v = (s as f32 / 32767.0) * gain;
            // Saturation douce (tanh) au-delà de ±1 — garde la tête propre.
            let v = if v.abs() > 1.0 { v.tanh() } else { v };
            let _ = w.write_sample((v.clamp(-1.0, 1.0) * 32767.0) as i16);
        }
        let _ = w.finalize();
    }
    if out.is_empty() {
        wav.to_vec()
    } else {
        out
    }
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
