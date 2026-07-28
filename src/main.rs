mod midi;
mod patterns;
mod render;
mod samples;
mod walking;

use axum::{extract::State, response::{Html, Json, IntoResponse}, routing::{get, post}, Router};
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use patterns::pat;
use midi::{apply_tracks, init_midi, note_midi, play_seq, play_notes, rch, pb, ChordEv, Live, LiveTrack, MidiHandle, TrackCfg as MidiTrackCfg, TRACK_BASS, TRACK_DRUMS, TRACK_LEAD, TRACK_STR};

#[derive(Clone)]
struct AppState { midi: Option<MidiHandle>, live: Arc<Live> }

// ─── Signature ──────────────────────────────────────────────────────────
fn sig_code(s: &str) -> u16 {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() != 2 { return 44 }
    let top: u16 = parts[0].parse().unwrap_or(4);
    let bot: u16 = parts[1].parse().unwrap_or(4);
    top * 10 + bot
}

// ─── Requêtes ──────────────────────────────────────────────────────────
#[derive(Deserialize)]
struct PlayReq {
    notes: Option<Vec<String>>,
    #[serde(default)] seq: Vec<ChordEv>,
    #[serde(default)] sequence: Vec<ChordEv>,
    #[serde(default = "t120")] tempo: u32,
    #[serde(default = "y")] drums: bool,
    #[serde(default = "y")] bass: bool,
    #[serde(default = "y")] arps: bool,
    #[serde(default = "n")] nappes: bool,
    #[serde(default = "s44")] sig: String,
    #[serde(default = "rk")] pattern: String,
    #[serde(default = "i51")] inst_val: u16,
    loop_enabled: Option<bool>,
    tracks: Option<Vec<MidiTrackCfg>>,
    walking: Option<bool>,
}

#[derive(Serialize)]
struct Rsp { status: String }

#[derive(Deserialize)]
struct Cfg {
    drums: Option<bool>, bass: Option<bool>, arpeggios: Option<bool>, nappes: Option<bool>,
    pattern: Option<String>, tempo: Option<u16>, sig: Option<String>, instrument: Option<u16>,
    tracks: Option<Vec<MidiTrackCfg>>,
    walking: Option<bool>,
    master_vol: Option<u8>,
    use432: Option<bool>,
    loop_offset: Option<i32>,
    use_loops: Option<bool>,
    loop_name: Option<String>,
    loop_volume: Option<u8>,
}

fn t120() -> u32 { 120 }
fn y() -> bool { true }
fn n() -> bool { false }
fn rk() -> String { "rock".to_string() }
fn s44() -> String { "4/4".to_string() }
fn i51() -> u16 { 51 }

// ─── Notes depuis ChordEv ──────────────────────────────────────────────
fn notes_from_ev(e: &ChordEv) -> Vec<u8> {
    let mut v = vec![];
    for n in &e.notes {
        if let Ok(x) = note_midi(n) { v.push(x) }
    }
    v
}

// ─── Routes ────────────────────────────────────────────────────────────
async fn idx() -> impl IntoResponse {
    Html(include_str!("../static/index.html"))
}

async fn play(State(s): State<AppState>, Json(b): Json<PlayReq>) -> impl IntoResponse {
    let lv = &s.live;
    if b.tempo > 0 { lv.tempo.store(b.tempo as u16, std::sync::atomic::Ordering::Relaxed) }
    lv.sig.store(sig_code(&b.sig), std::sync::atomic::Ordering::Relaxed);
    lv.pattern.store(pat(&b.pattern), std::sync::atomic::Ordering::Relaxed);
    lv.stop.store(false, std::sync::atomic::Ordering::Relaxed);
    if let Some(w) = b.walking { lv.walking.store(w, std::sync::atomic::Ordering::Relaxed) }
    if let Some(ref t) = b.tracks { apply_tracks(lv, t) }
    if b.tracks.is_none() {
        lv.tracks[TRACK_LEAD].program.store(b.inst_val, std::sync::atomic::Ordering::Relaxed);
        lv.tracks[TRACK_LEAD].mute.store(!b.arps, std::sync::atomic::Ordering::Relaxed);
        lv.tracks[TRACK_BASS].mute.store(!b.bass, std::sync::atomic::Ordering::Relaxed);
        lv.tracks[TRACK_STR].mute.store(!b.nappes, std::sync::atomic::Ordering::Relaxed);
        lv.tracks[TRACK_DRUMS].mute.store(!b.drums, std::sync::atomic::Ordering::Relaxed);
    }
    let do_loop = b.loop_enabled.unwrap_or(false);
    let ev: &[ChordEv] = if !b.seq.is_empty() { &b.seq } else if !b.sequence.is_empty() { &b.sequence } else { &[] };
    if let Some(ref h) = s.midi {
        let h2 = Arc::clone(h);
        let tempo_now = lv.tempo.load(std::sync::atomic::Ordering::Relaxed);
        let loop_active = lv.use_loops.load(std::sync::atomic::Ordering::Relaxed);
        if loop_active {
            let lname = lv.loop_name.lock().unwrap().clone();
            let name_opt = if lname.is_empty() { None } else { Some(lname.as_str()) };
            let lvol = lv.loop_volume.load(std::sync::atomic::Ordering::Relaxed);
            samples::set_volume(lvol);
            samples::play_loop(tempo_now, name_opt, lv.loop_offset.load(std::sync::atomic::Ordering::Relaxed));
        }
        if !ev.is_empty() {
            let sq = ev.to_vec();
            let l = Arc::clone(lv);
            std::thread::spawn(move || {
                if let Ok(mut c) = h2.lock() { play_seq(&mut c, &sq, &l, do_loop) }
            });
        } else if let Some(ref n) = b.notes {
            let v = n.clone();
            let l2 = Arc::clone(lv);
            std::thread::spawn(move || {
                let mv = l2.master_vol.load(std::sync::atomic::Ordering::Relaxed);
                if let Ok(mut c) = h2.lock() { play_notes(&mut c, &v, mv) }
            });
        }
    }
    Json(Rsp { status: "ok".into() })
}

async fn conf(State(s): State<AppState>, Json(b): Json<Cfg>) -> impl IntoResponse {
    let lv = &s.live;
    if let Some(ref t) = b.tracks { apply_tracks(lv, t) }
    if let Some(v) = b.drums { lv.tracks[TRACK_DRUMS].mute.store(!v, std::sync::atomic::Ordering::Relaxed) }
    if let Some(v) = b.bass { lv.tracks[TRACK_BASS].mute.store(!v, std::sync::atomic::Ordering::Relaxed) }
    if let Some(v) = b.arpeggios { lv.tracks[TRACK_LEAD].mute.store(!v, std::sync::atomic::Ordering::Relaxed) }
    if let Some(v) = b.nappes { lv.tracks[TRACK_STR].mute.store(!v, std::sync::atomic::Ordering::Relaxed) }
    if let Some(ref p) = b.pattern { lv.pattern.store(pat(p), std::sync::atomic::Ordering::Relaxed) }
    if let Some(t) = b.tempo { lv.tempo.store(t, std::sync::atomic::Ordering::Relaxed); samples::set_current_tempo(t); }
    if let Some(ref sg) = b.sig { lv.sig.store(sig_code(sg), std::sync::atomic::Ordering::Relaxed) }
    if let Some(iv) = b.instrument { lv.tracks[TRACK_LEAD].program.store(iv, std::sync::atomic::Ordering::Relaxed) }
    if let Some(w) = b.walking { lv.walking.store(w, std::sync::atomic::Ordering::Relaxed) }
    if let Some(m) = b.master_vol { lv.master_vol.store(m, std::sync::atomic::Ordering::Relaxed); }
    if let Some(u) = b.use432 {
        let was = lv.use432.swap(u, std::sync::atomic::Ordering::Relaxed);
        if was != u {
            if let Some(ref h) = s.midi {
                if let Ok(mut c) = h.lock() {
                    for &ch in &[0u8, 2, 3, 4] { pb(&mut c, ch, if u { 6881 } else { 8192 }) }
                }
            }
        }
    }
    if let Some(off) = b.loop_offset { lv.loop_offset.store(off, std::sync::atomic::Ordering::Relaxed); samples::update_offset(off); }
    if let Some(lo) = b.use_loops { lv.use_loops.store(lo, std::sync::atomic::Ordering::Relaxed); samples::set_use_loops(lo); }
    if let Some(ref n) = b.loop_name { *lv.loop_name.lock().unwrap() = n.clone(); }
    if let Some(lv2) = b.loop_volume { lv.loop_volume.store(lv2, std::sync::atomic::Ordering::Relaxed); samples::set_volume(lv2); }
    Json(Rsp { status: "ok".into() })
}

async fn stop(State(s): State<AppState>) -> impl IntoResponse {
    s.live.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    samples::stop_loop();
    if let Some(ref h) = s.midi {
        if let Ok(mut c) = h.lock() { rch(&mut c) }
    }
    Json(serde_json::json!({"status": "stopped"}))
}

async fn render_wav(Json(b): Json<PlayReq>) -> impl IntoResponse {
    use axum::http::{HeaderMap, StatusCode};

    let ev: &[ChordEv] = if !b.seq.is_empty() { &b.seq } else if !b.sequence.is_empty() { &b.sequence } else { &[] };
    if ev.is_empty() {
        return (StatusCode::BAD_REQUEST, "Empty sequence").into_response();
    }

    let mut notes_arrays: Vec<Vec<u8>> = Vec::new();
    let mut beats: Vec<f64> = Vec::new();
    for e in ev {
        notes_arrays.push(notes_from_ev(e));
        beats.push(e.beats);
    }

    let mut tracks_cfg: [render::TrackCfg; 5] = [
        render::TrackCfg { channel: 0, program: b.inst_val, volume: 15, mute: !b.arps },
        render::TrackCfg { channel: 2, program: 33, volume: 40, mute: !b.bass },
        render::TrackCfg { channel: 3, program: 48, volume: 30, mute: !b.nappes },
        render::TrackCfg { channel: 9, program: 1, volume: 80, mute: !b.drums },
        render::TrackCfg { channel: 4, program: 2, volume: 20, mute: false },
    ];
    if let Some(ref tcfg) = b.tracks {
        for tc in tcfg {
            if let Some(t) = tracks_cfg.iter_mut().find(|t| t.channel == tc.channel) {
                t.program = tc.program.unwrap_or(t.program);
                t.volume = tc.volume.unwrap_or(t.volume);
                t.mute = tc.mute.unwrap_or(t.mute);
            }
        }
    }

    let rcfg = render::RenderCfg {
        tempo: b.tempo,
        pattern: b.pattern.clone(),
        walking: b.walking.unwrap_or(false),
        sig: b.sig.clone(),
        lead_inst: b.inst_val,
        tracks: tracks_cfg,
    };

    let smf = render::generate_smf_fmt0(&notes_arrays, &beats, &rcfg);
    let sf_path = "/usr/share/sounds/sf3/MuseScore_General_Full.sf3";
    let total_beats: f64 = beats.iter().sum();
    let duration_sec = total_beats * 60.0 / b.tempo.max(1) as f64;

    match render::render_wav(&smf, sf_path, duration_sec) {
        Ok(wav) => {
            let mut h = HeaderMap::new();
            h.insert("Content-Type", "audio/wav".parse().unwrap());
            (StatusCode::OK, h, wav).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

#[tokio::main]
async fn main() {
    println!("csrs");
    let midi = init_midi();
    samples::init();
    use patterns::PAT_ROCK;
    use std::sync::atomic::{AtomicU16, AtomicU8, AtomicI32};
    use std::sync::Mutex;

    let state = AppState {
        midi,
        live: Arc::new(Live {
            tracks: [
                LiveTrack::new(0, 51, 15),
                LiveTrack::new(2, 33, 40),
                LiveTrack::new(3, 48, 30),
                LiveTrack::new(9, 1, 80),
                LiveTrack::new(4, 2, 20),
            ],
            pattern: AtomicU8::new(PAT_ROCK),
            tempo: AtomicU16::new(120),
            stop: AtomicBool::new(false),
            sig: AtomicU16::new(44),
            walking: AtomicBool::new(false),
            master_vol: AtomicU8::new(127),
            use432: AtomicBool::new(false),
            loop_offset: AtomicI32::new(0),
            use_loops: AtomicBool::new(false),
            loop_name: Mutex::new(String::new()),
            loop_volume: AtomicU8::new(80),
        }),
    };

    async fn samples_list() -> impl IntoResponse {
        use axum::http::StatusCode;
        let data = samples::get_available();
        (StatusCode::OK, axum::Json(data))
    }

    let app = Router::new()
        .route("/", get(idx))
        .route("/play", post(play))
        .route("/config", post(conf))
        .route("/stop", post(stop))
        .route("/render-wav", post(render_wav))
        .route("/samples-list", get(samples_list))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let p = std::env::var("PORT").unwrap_or_else(|_| "4000".to_string());
    println!("http://0.0.0.0:{p}");
    let l = tokio::net::TcpListener::bind(format!("0.0.0.0:{p}")).await.unwrap();
    axum::serve(l, app).await.unwrap();
}
