use axum::{extract::State, response::{Html, Json, IntoResponse}, routing::{get, post}, Router};
use midir::{MidiOutput, MidiOutputConnection};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower_http::cors::CorsLayer;

type MidiHandle = Arc<Mutex<MidiOutputConnection>>;
#[derive(Clone)] struct AppState { midi: Option<MidiHandle> }

#[derive(Deserialize)]
struct PlayReq {
    notes: Option<Vec<String>>,
    #[serde(default)] seq: Vec<ChordEv>,
    #[serde(default)] sequence: Vec<ChordEv>,
    #[serde(default = "t120")] tempo: u32,
}
#[derive(Deserialize, Clone)] struct ChordEv { notes: Vec<String>, #[serde(default = "b4")] beats: f64 }
#[derive(Serialize)] struct Rsp { status: String, notes: Vec<String> }
fn t120() -> u32 { 120 } fn b4() -> f64 { 4.0 }

fn note_midi(s: &str) -> Result<u8, String> {
    let s = s.trim(); let (nl, np) = if s.len() > 1 && (s.as_bytes()[1] == b'#' || s.as_bytes()[1] == b'b') { (2, &s[..2]) } else { (1, &s[..1]) };
    let o: i32 = s[nl..].parse().map_err(|_| "oct")?; let u = np.to_uppercase();
    let n = match u.as_str() { "DB" => "C#", "EB" => "D#", "GB" => "F#", "AB" => "G#", "BB" => "A#", _ => &u };
    let i = ["C","C#","D","D#","E","F","F#","G","G#","A","A#","B"].iter().position(|x| x == &n).ok_or("?")?;
    let m = (o + 1) * 12 + i as i32; if m < 0 || m > 127 { return Err("out".into()) } Ok(m as u8)
}

fn init_midi() -> Option<MidiHandle> {
    let mo = MidiOutput::new("cs").ok()?; let p = mo.ports();
    if p.is_empty() { eprintln!("no port"); return None }
    println!("Ports:"); for (i, x) in p.iter().enumerate() { if let Ok(n) = mo.port_name(x) { println!(" [{i}] {n}") } }
    let i: usize = if let Ok(e) = std::env::var("MIDI_PORT") { e.parse().unwrap_or(0) }
    else if let Some((i,_)) = p.iter().enumerate().find(|(_,x)| mo.port_name(x).map(|n| n.contains("Roland")).unwrap_or(false)) { println!("  Roland"); i }
    else { p.len().saturating_sub(1) };
    if i >= p.len() { eprintln!("port {i} invalid"); return None }
    println!("Connecte {}", mo.port_name(&p[i]).unwrap_or_default());
    mo.connect(&p[i], "cs").ok().map(|c| Arc::new(Mutex::new(c)))
}

fn snd(c: &mut MidiOutputConnection, m: &[u8]) { if let Err(e) = c.send(m) { eprintln!("⚠️{e}") } }
fn cc(c: &mut MidiOutputConnection, ch: u8, ctl: u8, v: u8) { snd(c, &[0xB0 | ch, ctl, v]) }
fn pc(c: &mut MidiOutputConnection, ch: u8, v: u8) { snd(c, &[0xC0 | ch, v]) }
fn no(c: &mut MidiOutputConnection, ch: u8, n: u8, v: u8) { snd(c, &[0x90 | ch, n, v]) }

fn reset_ch(c: &mut MidiOutputConnection) { for &ch in &[0u8, 1, 2, 9] { cc(c, ch, 123, 0) } }

fn play_notes(c: &mut MidiOutputConnection, notes: &[String]) {
    let mut v: Vec<u8> = vec![]; for n in notes { if let Ok(m) = note_midi(n) { v.push(m) } } if v.is_empty() { return }
    reset_ch(c);
    for &ch in &[0u8, 1, 2] { cc(c, ch, 101, 0); cc(c, ch, 100, 1); cc(c, ch, 6, 62); cc(c, ch, 38, 2) }
    pc(c, 0, 51); pc(c, 1, 24); pc(c, 2, 50);
    for _ in 0..2 { for &n in &v { std::thread::sleep(Duration::from_millis(240)); if n < 48 { no(c, 2, n, 35) } else { no(c, 0, n, 15); no(c, 1, n, 15) } } }
    reset_ch(c); println!("  notes: {v:?}");
}

fn play_seq(c: &mut MidiOutputConnection, ev: &[ChordEv], tempo: u32) {
    reset_ch(c); // noteoff au début
    let beat_ms = 60_000.0 / tempo as f64;
    for &ch in &[0u8, 1, 2] { cc(c, ch, 101, 0); cc(c, ch, 100, 1); cc(c, ch, 6, 62); cc(c, ch, 38, 2) }
    pc(c, 0, 51); pc(c, 1, 24); pc(c, 2, 50);

    for (i, e) in ev.iter().enumerate() {
        let mut m: Vec<u8> = vec![]; for n in &e.notes { if let Ok(x) = note_midi(n) { m.push(x) } }
        if m.is_empty() { continue }
        if i > 0 { reset_ch(c) }

        let root = m[0]; let arp: Vec<u8> = m[1..].to_vec();
        let dur = (beat_ms * e.beats) as u64;
        let start = std::time::Instant::now();
        let delay = (beat_ms / 4.0).max(30.0) as u64;
        let mut idx = 0u64;

        while (start.elapsed().as_millis() as u64) < dur {
            let target = start + Duration::from_millis(idx * delay);
            let now = std::time::Instant::now();
            if target > now { std::thread::sleep(target - now) }

            let tick = idx % arp.len().max(1) as u64;
            if !arp.is_empty() {
                if idx > 0 { let p = arp[((idx - 1) % arp.len() as u64) as usize]; no(c, 0, p, 0); no(c, 1, p, 0); }
                no(c, 0, arp[tick as usize], 15); no(c, 1, arp[tick as usize], 15);
            }
            if idx % 4 == 0 { if idx >= 4 { no(c, 2, root, 0) } no(c, 2, root, 40); }
            idx += 1;
        }
    }
    reset_ch(c); // noteoff à la fin
    println!("  done ({} accords)", ev.len());
}

async fn idx() -> impl IntoResponse { Html(include_str!("../static/index.html")) }

async fn play(State(s): State<AppState>, Json(b): Json<PlayReq>) -> impl IntoResponse {
    let events: &[ChordEv] = if !b.seq.is_empty() { b.seq.as_slice() } else if !b.sequence.is_empty() { b.sequence.as_slice() } else { &[] };
    if let Some(ref h) = s.midi {
        let h2 = Arc::clone(h);
        if !events.is_empty() { let sq = events.to_vec(); let t = b.tempo;
            std::thread::spawn(move || { if let Ok(mut c) = h2.lock() { play_seq(&mut c, &sq, t) } }); }
        else if let Some(ref n) = b.notes { let v = n.clone();
            std::thread::spawn(move || { if let Ok(mut c) = h2.lock() { play_notes(&mut c, &v) } }); }
    }
    Json(Rsp { status: "ok".into(), notes: vec![] })
}

#[tokio::main]
async fn main() {
    println!("csrs"); let midi = init_midi();
    let state = AppState { midi };
    let app = Router::new().route("/", get(idx)).route("/play", post(play)).layer(CorsLayer::permissive()).with_state(state);
    let p = std::env::var("PORT").unwrap_or_else(|_| "4000".to_string());
    println!("http://0.0.0.0:{p}");
    let l = tokio::net::TcpListener::bind(format!("0.0.0.0:{p}")).await.unwrap();
    axum::serve(l, app).await.unwrap();
}
