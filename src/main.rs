mod render;
mod samples;
mod synth;
use axum::{extract::ws::{Message, WebSocket, WebSocketUpgrade}, extract::State, response::{Html, Json, IntoResponse}, routing::{get, post}, Router};
use futures_util::SinkExt;
use midir;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU16, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower_http::cors::CorsLayer;
use synth::{SynthOut, FluidSynthRenderer as SynthRenderer, MultiOut, AudioBuffer};

type MidiHandle = Arc<Mutex<midir::MidiOutputConnection>>;

// ─── Tones ──────────────────────────────────────────────────────────────
const MIN_NOTE:u8=22; const MAX_NOTE:u8=42;

// ─── Tracks ──────────────────────────────────────────────────────────────
const TRACK_LEAD:usize=0;
const TRACK_BASS:usize=1;
const TRACK_STR:usize=2;
const TRACK_DRUMS:usize=3;
const TRACK_ACCENT:usize=4;

struct LiveTrack {
    channel: u8,
    program: AtomicU16,
    volume: AtomicU8,
    mute: AtomicBool,
}

impl LiveTrack {
    fn new(ch: u8, pg: u16, vol: u8) -> Self {
        Self { channel: ch, program: AtomicU16::new(pg), volume: AtomicU8::new(vol), mute: AtomicBool::new(false) }
    }
}

struct Live {
    tracks: [LiveTrack; 5],
    pattern: AtomicU8,
    tempo: AtomicU16,
    stop: AtomicBool,
    sig: AtomicU16,
    walking: AtomicBool,
    master_vol: AtomicU8,
    use432: AtomicBool,
    loop_offset: AtomicI32,
    use_loops: AtomicBool,
    loop_name: Mutex<String>,
    loop_volume: AtomicU8,
}

#[derive(Clone)] struct AppState{midi:Option<MidiHandle>,live:Arc<Live>,synth:Option<Arc<Mutex<SynthRenderer>>>,audio_buffer:Option<AudioBuffer>}

// ─── Patterns ────────────────────────────────────────────────────────────
const PAT_ROCK:u8=0; const PAT_REGGAE:u8=1; const PAT_JAZZ:u8=2;
const PAT_POP:u8=3; const PAT_BOSSA:u8=4; const PAT_ONEDROP:u8=5;
fn pat(s:&str)->u8{match s{"reggae"=>PAT_REGGAE,"jazz"=>PAT_JAZZ,"pop"=>PAT_POP,"bossa"=>PAT_BOSSA,"onedrop"=>PAT_ONEDROP,_=>PAT_ROCK}}
fn sig_code(s:&str)->u16{
    let parts:Vec<&str>=s.split('/').collect();
    if parts.len()!=2{return 44}let top:u16=parts[0].parse().unwrap_or(4);let bot:u16=parts[1].parse().unwrap_or(4);top*10+bot
}

// ─── Requetes ────────────────────────────────────────────────────────────
#[derive(Deserialize)]
struct TrackCfg {
    channel: u8,
    program: Option<u16>,
    volume: Option<u8>,
    mute: Option<bool>,
}

#[derive(Deserialize)]
struct PlayReq{
    notes:Option<Vec<String>>,#[serde(default)]seq:Vec<ChordEv>,#[serde(default)]sequence:Vec<ChordEv>,
    #[serde(default="t120")]tempo:u32,#[serde(default="y")]drums:bool,
    #[serde(default="y")]bass:bool,#[serde(default="y")]arps:bool,#[serde(default="n")]nappes:bool,
    #[serde(default="s44")]sig:String,#[serde(default="rk")]pattern:String,#[serde(default="i51")]inst_val:u16,
    loop_enabled:Option<bool>,
    tracks:Option<Vec<TrackCfg>>,
    walking:Option<bool>,
}

#[derive(Deserialize,Clone)] struct ChordEv{notes:Vec<String>,#[serde(default="b4")]beats:f64}
#[derive(Serialize)] struct Rsp{status:String}

#[derive(Deserialize)]
struct Cfg{
    drums:Option<bool>,bass:Option<bool>,arpeggios:Option<bool>,nappes:Option<bool>,
    pattern:Option<String>,tempo:Option<u16>,sig:Option<String>,instrument:Option<u16>,
    tracks:Option<Vec<TrackCfg>>,
    walking:Option<bool>,
    master_vol:Option<u8>,
    use432:Option<bool>,
    loop_offset:Option<i32>,
    use_loops:Option<bool>,
    loop_name:Option<String>,
    loop_volume:Option<u8>,
}

fn t120()->u32{120}fn y()->bool{true}fn n()->bool{false}fn rk()->String{"rock".to_string()}fn s44()->String{"4/4".to_string()}fn i51()->u16{51}fn b4()->f64{4.0}

// ─── Utilitaire MIDI ─────────────────────────────────────────────────────
fn note_midi(s:&str)->Result<u8,String>{
    let s=s.trim();let(nl,np)=if s.len()>1&&(s.as_bytes()[1]==b'#'||s.as_bytes()[1]==b'b'){(2,&s[..2])}else{(1,&s[..1])};
    let o:i32=s[nl..].parse().map_err(|_|"o")?;let u=np.to_uppercase();
    let n=match u.as_str(){"DB"=>"C#","EB"=>"D#","GB"=>"F#","AB"=>"G#","BB"=>"A#",_=>&u};
    let i=["C","C#","D","D#","E","F","F#","G","G#","A","A#","B"].iter().position(|x|x==&n).ok_or("?")?;
    let m=(o+1)*12+i as i32;if m<0||m>127{return Err("o".into())}Ok(m as u8)
}

fn notes_from_ev(e:&ChordEv)->Vec<u8>{
    let mut v=vec![];for n in &e.notes{if let Ok(x)=note_midi(n){v.push(x)}}v
}

/// Determine si un accord est mineur (tierce mineure entre fondamentale et 3eme note)
fn is_minor(chord: &ChordEv) -> bool {
    let midi = notes_from_ev(chord);
    if midi.len() < 3 { return false; }
    let root = midi[1];
    let mut has_minor = false;
    let mut has_major = false;
    for &n in &midi[1..] {
        let interval = if n >= root { n - root } else { n + 12 - root };
        match interval {
            3 => has_minor = true,
            4 => has_major = true,
            _ => {}
        }
    }
    has_minor && !has_major
}

fn init_midi()->Option<MidiHandle>{
    use midir::MidiOutput;
    let mo=MidiOutput::new("cs").ok()?;let p=mo.ports();
    if p.is_empty(){eprintln!("no port");return None}
    println!("Ports:");for(i,x)in p.iter().enumerate(){if let Ok(n)=mo.port_name(x){println!(" [{i}] {n}")}}
    let i:usize=if let Ok(e)=std::env::var("MIDI_PORT"){e.parse().unwrap_or(2)}else{2};
    if i>=p.len(){eprintln!("port {i} invalid");return None}
    println!("Connecte {}",mo.port_name(&p[i]).unwrap_or_default());
    mo.connect(&p[i],"cs").ok().map(|c|Arc::new(Mutex::new(c)))
}



// ─── Walking Bass ────────────────────────────────────────────────────────
fn bass_clamp(n: u8) -> u8 {
    if n < MIN_NOTE { n + 12 }
    else if n > MAX_NOTE { n - 12 }
    else { n }
}

/// Genere 4 notes de walking bass pour une mesure (4 temps)
fn generate_walking_bass(current_notes: &[u8], next_root: u8, seed: u64, minor: bool) -> [u8; 4] {
    let root = current_notes[0];
    let chord_tones: Vec<u8> = if current_notes.len() > 1 {
        current_notes[1..].iter().map(|&n| bass_clamp(n)).collect()
    } else {
        vec![root - 5]
    };
    let mut tones: Vec<u8> = chord_tones.clone();
    tones.sort();
    tones.dedup();
    let tones = tones;

    let b1 = root;

    let b2 = if minor {
        match seed % 100 {
            0..=24 => root + 2,
            25..=49 => root - 10,
            _ => { let idx2 = (seed as usize) % tones.len(); tones[idx2] }
        }
    } else {
        let idx2 = (seed as usize) % tones.len();
        tones[idx2]
    };

    let filtered: Vec<u8> = tones.iter().filter(|&&n| n != b2).copied().collect();
    let b3 = if filtered.is_empty() { b2 + 7 } else { filtered[(seed.wrapping_add(7) as usize) % filtered.len()] };

    let b4 = match (seed % 100) as u8 {
        0..=49 => {
            let dir = if seed % 2 == 0 { 1i8 } else { -1i8 };
            let mut app = (next_root as i16 + dir as i16) as u8;
            if app < MIN_NOTE || app > MAX_NOTE { app = (next_root as i16 - dir as i16) as u8; }
            if app < MIN_NOTE { app = next_root + 12; }
            if app > MAX_NOTE { app = next_root - 12; }
            app
        }
        50..=67 => { let mut app = next_root + 7; if app > MAX_NOTE { app -= 12; } app }
        68..=85 => { let mut app = next_root - 5; if app < MIN_NOTE { app += 12; } app }
        _ => { tones.iter().min_by_key(|&&t| { let diff = if t > next_root { t - next_root } else { next_root - t }; diff }).copied().unwrap_or(next_root) }
    };

    [b1, b2, b3, b4]
}

// ─── MIDI helpers ───────────────────────────────────────────────────────
const DRUM_KICK:u8=36; const DRUM_SNARE:u8=38; const DRUM_RIM:u8=37;
const DRUM_HH:u8=42; const DRUM_RIDE:u8=51;
const HH_BEAT:u8=80; const HH_8TH:u8=65;
fn scale_mv(v:u8,mv:u8)->u8{((v as u16*mv as u16)/127).min(127)as u8}

fn drum_hit(midi:&mut impl SynthOut,beat:u64,pat:u8,on_beat:bool,on_eighth:bool,bars:u64,vol:u8,mv:u8){
    if!on_beat&&!on_eighth{return}
    let b=beat%bars;
    let v=scale_mv(vol,mv);
    let hh=vscale(v,HH_BEAT);let h8=vscale(v,HH_8TH);let h55=vscale(v,55);let h45=vscale(v,45);let h40=vscale(v,10);
    let h60=vscale(v,60);let h65=vscale(v,65);
    match pat{
        PAT_REGGAE=>if on_beat{match b{
            0=>{midi.note_on(9,DRUM_HH,h60);}
            1=>{midi.note_on(9,DRUM_HH,h60);}
            2=>{midi.note_on(9,DRUM_KICK,vscale(v,120));midi.note_on(9,DRUM_HH,h65);midi.note_on(9,DRUM_RIM,vscale(v,90));}
            3=>{midi.note_on(9,DRUM_HH,h60);}_=>{}
        }}else if on_eighth{midi.note_on(9,DRUM_HH,h40);}
        PAT_JAZZ=>{
            let b=beat%8;
            if on_beat{match b{
                0=>{midi.note_on(9,DRUM_RIDE,h60);}
                2=>{midi.note_on(9,DRUM_RIDE,h60);}
                4=>{midi.note_on(9,DRUM_RIDE,h60);midi.note_on(9,44,vscale(v,40));}
                6=>{midi.note_on(9,DRUM_RIDE,h60);}
                7=>{midi.note_on(9,DRUM_RIDE,h60);midi.note_on(9,44,vscale(v,40));midi.note_on(9,DRUM_RIM,vscale(v,50));}_=>{}
            }}else if on_eighth{midi.note_on(9,DRUM_HH,35);}
        }
        PAT_POP=>{
            let b=beat%8;
            if on_beat{match b{
            0=>{midi.note_on(9,DRUM_KICK,vscale(v,85));midi.note_on(9,DRUM_HH,vscale(v,50));}
            2=>{midi.note_on(9,DRUM_SNARE,vscale(v,70));midi.note_on(9,DRUM_HH,vscale(v,50));}
            4=>{midi.note_on(9,DRUM_KICK,vscale(v,75));midi.note_on(9,DRUM_HH,vscale(v,50));}
            6=>{midi.note_on(9,DRUM_SNARE,vscale(v,65));midi.note_on(9,DRUM_HH,vscale(v,50));}_=>{}
        }}else if on_eighth{midi.note_on(9,DRUM_HH,vscale(v,45));}
    }
        PAT_BOSSA=>if on_beat{match b{
            0=>{midi.note_on(9,DRUM_KICK,vscale(v,55));midi.note_on(9,DRUM_HH,h45);}
            1=>{midi.note_on(9,DRUM_SNARE,vscale(v,30));midi.note_on(9,DRUM_HH,h45);}
            2=>{midi.note_on(9,DRUM_KICK,vscale(v,60));midi.note_on(9,DRUM_HH,h45);}
            3=>{midi.note_on(9,DRUM_KICK,vscale(v,50));midi.note_on(9,DRUM_HH,h45);}_=>{}
        }}else if on_eighth{midi.note_on(9,DRUM_HH,h40);}
        PAT_ONEDROP=>if on_beat{match b{
            0=>{midi.note_on(9,DRUM_KICK,vscale(v,90));midi.note_on(9,DRUM_HH,h55);}
            1=>{midi.note_on(9,DRUM_HH,h40);}
            2=>{midi.note_on(9,DRUM_KICK,vscale(v,90));midi.note_on(9,DRUM_RIM,vscale(v,65));midi.note_on(9,DRUM_HH,h45);}
            3=>{midi.note_on(9,DRUM_HH,h55);}_=>{}
        }}else if on_eighth{midi.note_on(9,DRUM_HH,h40);}
        _=>if on_beat{match b{
            0=>{midi.note_on(9,DRUM_KICK,vscale(v,90));midi.note_on(9,DRUM_HH,hh);}
            1=>{midi.note_on(9,DRUM_SNARE,vscale(v,75));midi.note_on(9,DRUM_HH,hh);}
            2=>{midi.note_on(9,DRUM_KICK,vscale(v,80));midi.note_on(9,DRUM_HH,hh);}
            3=>{midi.note_on(9,DRUM_SNARE,vscale(v,70));midi.note_on(9,DRUM_HH,hh);}_=>{}
        }}else if on_eighth{midi.note_on(9,DRUM_HH,h8);}
    }
}
fn vscale(vol:u8,base:u8)->u8{((vol as u16*base as u16)/127).min(127) as u8}

fn play_notes(midi:&mut impl SynthOut,notes:&[String],mv:u8){
    let mut v:Vec<u8>=vec![];for n in notes{if let Ok(m)=note_midi(n){v.push(m)}}if v.is_empty(){return}
    midi.all_notes_off();
    for&ch in&[0u8,2,3]{midi.control_change(ch,101,0);midi.control_change(ch,100,1);midi.control_change(ch,6,62);midi.control_change(ch,38,2)}
    midi.program_change(0,51);midi.program_change(2,33);
    for _ in 0..2{for&n in&v{std::thread::sleep(Duration::from_millis(240));
        let vel=if n<MIN_NOTE{scale_mv(35,mv)}else{scale_mv(15,mv)};
        midi.note_on(if n<MIN_NOTE{2}else{0},n,vel);}}
    midi.all_notes_off();println!("  notes: {v:?}");
}

fn setup_tracks(midi:&mut impl SynthOut,lc:&Live){
    for t in &lc.tracks {
        let ch=t.channel;
        if ch==9{midi.program_change(ch,1);continue}
        midi.program_change(ch,t.program.load(Ordering::Relaxed)as u8);
    }
}

fn play_seq(midi:&mut impl SynthOut,ev:&[ChordEv],lc:&Live,do_loop:bool){
    loop {
        midi.all_notes_off();
        setup_tracks(midi,lc);
        std::thread::sleep(Duration::from_millis(2));

        let mut prev_nappe:Vec<u8>=vec![];
        let mut prev_lead:Vec<u8>=vec![];
        let mut prev_accent:Vec<u8>=vec![];
        let t_lead=&lc.tracks[TRACK_LEAD];
        let t_bass=&lc.tracks[TRACK_BASS];
        let t_str=&lc.tracks[TRACK_STR];
        let t_drums=&lc.tracks[TRACK_DRUMS];
        let ch_lead=t_lead.channel;
        let ch_bass=t_bass.channel;
        let ch_str=t_str.channel;
        let t_accent=&lc.tracks[TRACK_ACCENT];
        let ch_accent=t_accent.channel;
        let walking=lc.walking.load(Ordering::Relaxed);
        let mv=lc.master_vol.load(Ordering::Relaxed);
        let mut seed:u64 = 0;

        for(i,e)in ev.iter().enumerate(){
            let mut m:Vec<u8>=vec![];for n in&e.notes{if let Ok(x)=note_midi(n){m.push(x)}}
            if m.is_empty(){
                if i>0{midi.all_notes_off();midi.control_change(9,120,0)}
                let dur=(60_000.0/lc.tempo.load(Ordering::Relaxed).max(20)as f64*e.beats)as u64;
                let start=std::time::Instant::now();
                let mut idx=0u64;
                let mut last_b_drums=u64::MAX;
                let dur_f = dur as f64; while start.elapsed().as_secs_f64() * 1000.0 < dur_f&&!lc.stop.load(Ordering::Relaxed){
                    let tempo_f=lc.tempo.load(Ordering::Relaxed).max(20)as f64;
                    let bd_ms=60_000.0/tempo_f;
                    let delay_ms=(bd_ms/4.0).max(30.0);
                    let target=start+Duration::from_secs_f64(idx as f64*delay_ms/1000.0);
                    let now=std::time::Instant::now();
                    if target>now{std::thread::sleep(target-now)}
                    let elapsed_ms=start.elapsed().as_secs_f64()*1000.0;
                    let pt=lc.pattern.load(Ordering::Relaxed);
                    let sig=lc.sig.load(Ordering::Relaxed);
                    let bars=(sig/10).max(1)as u64;
                    let beat=(elapsed_ms/bd_ms)as u64;
                    if !t_drums.mute.load(Ordering::Relaxed){
                        if last_b_drums==u64::MAX||beat>last_b_drums{
                            let dvol=scale_mv(t_drums.volume.load(Ordering::Relaxed),mv);
                            drum_hit(midi,beat,pt,true,false,bars,dvol,127);
                            last_b_drums=beat;
                        }
                        let beat_pos=elapsed_ms%bd_ms;
                        if beat_pos>bd_ms/2.0-10.0&&beat_pos<bd_ms/2.0+10.0{
                            let dvol=scale_mv(t_drums.volume.load(Ordering::Relaxed),mv);
                            drum_hit(midi,beat,pt,false,true,bars,dvol,127);
                        }
                    }
                    midi.render_audio((delay_ms as f64 * 44.1) as usize);
                    idx+=1;
                }
                if lc.stop.load(Ordering::Relaxed){break}
                continue;
            }
            if i>0{midi.all_notes_off();midi.control_change(9,120,0)}

            let root=m[0];
            let nappe_notes:Vec<u8>=m[1..].to_vec();
            let dur=(60_000.0/lc.tempo.load(Ordering::Relaxed).max(20)as f64*e.beats)as u64;
            let start=std::time::Instant::now();
            let mut idx=0u64;
            let mut last_b_drums=u64::MAX;
            let mut last_b_bass=0u64;
            let mut prev_bass_note:u8=0;

            let mut walking_notes:[u8;4]=[root,root,root,root];
            if walking && PAT_REGGAE != lc.pattern.load(Ordering::Relaxed) {
                let next_root = if let Some(ne) = ev.get(i+1) {
                    let nv = notes_from_ev(ne);
                    if !nv.is_empty() { nv[0] } else { root }
                } else {
                    if let Some(ne) = ev.get(0) {
                        let nv = notes_from_ev(ne);
                        if !nv.is_empty() { nv[0] } else { root }
                    } else { root }
                };
                walking_notes = generate_walking_bass(&m, next_root, seed, is_minor(e));
                seed = seed.wrapping_add(1);
            }

            if !t_bass.mute.load(Ordering::Relaxed) {
                let bvol=scale_mv(t_bass.volume.load(Ordering::Relaxed),mv);
                let bass_note = if walking { walking_notes[0] } else { root };
                midi.note_on(ch_bass,bass_note,((bvol as u16*mv as u16)/127).min(127) as u8);
                prev_bass_note=bass_note;
                last_b_bass=0;
            }

            if !t_str.mute.load(Ordering::Relaxed) {
                for n in &prev_nappe{midi.note_off(ch_str,*n)}
                let str_vol=scale_mv(t_str.volume.load(Ordering::Relaxed),mv);
                for n in &nappe_notes{midi.note_on(ch_str,*n,((str_vol as u16*mv as u16)/127).min(127) as u8)}
                prev_nappe=nappe_notes.clone();
            }

            let dur_f = dur as f64; while start.elapsed().as_secs_f64() * 1000.0 < dur_f&&!lc.stop.load(Ordering::Relaxed){
                let tempo_f=lc.tempo.load(Ordering::Relaxed).max(20)as f64;
                let bd_ms=60_000.0/tempo_f;
                let delay_ms=(bd_ms/4.0).max(30.0);

                let target=start+Duration::from_secs_f64(idx as f64*delay_ms/1000.0);
                let now=std::time::Instant::now();
                if target>now{std::thread::sleep(target-now)}

                let elapsed_ms=start.elapsed().as_secs_f64()*1000.0;
                let pt=lc.pattern.load(Ordering::Relaxed);
                let sig=lc.sig.load(Ordering::Relaxed);
                let bars=(sig/10).max(1)as u64;
                let beat=(elapsed_ms/bd_ms)as u64;

                if !t_drums.mute.load(Ordering::Relaxed){
                    if last_b_drums==u64::MAX||beat>last_b_drums{
                        let dvol=scale_mv(t_drums.volume.load(Ordering::Relaxed),mv);
                        drum_hit(midi,beat,pt,true,false,bars,dvol,127);
                        last_b_drums=beat;
                    }
                    let beat_pos=elapsed_ms%bd_ms;
                    if beat_pos>bd_ms/2.0-10.0&&beat_pos<bd_ms/2.0+10.0{
                        let dvol=scale_mv(t_drums.volume.load(Ordering::Relaxed),mv);
                        drum_hit(midi,beat,pt,false,true,bars,dvol,127);
                    }
                }

                if !t_bass.mute.load(Ordering::Relaxed){
                    if beat>last_b_bass{
                        let bvol=scale_mv(t_bass.volume.load(Ordering::Relaxed),mv);
                        let bass_note = if walking {
                            let bi = (beat % 4) as usize;
                            walking_notes[bi]
                        } else {
                            root
                        };
                        midi.note_off(ch_bass,prev_bass_note);
                        midi.note_on(ch_bass,bass_note,((bvol as u16*mv as u16)/127).min(127) as u8);
                        prev_bass_note=bass_note;
                        last_b_bass=beat;
                    }
                }

                if !m.is_empty(){
                    let lead_mute=t_lead.mute.load(Ordering::Relaxed);
                    if idx%4==3&&!prev_lead.is_empty(){
                        for &n in &prev_lead{midi.note_off(ch_lead,n)}
                        prev_lead.clear();
                    }
                    if idx%4==2&&!lead_mute{
                        let lvol=scale_mv(t_lead.volume.load(Ordering::Relaxed),mv);
                        prev_lead=m.clone();
                        for &note in &m{midi.note_on(ch_lead,note,((lvol as u16*mv as u16)/127).min(127) as u8)}
                    }
                }

                if !m.is_empty(){
                    let accent_mute=t_accent.mute.load(Ordering::Relaxed);
                    if idx%8==5&&!prev_accent.is_empty(){
                        for &n in &prev_accent{midi.note_off(ch_accent,n)}
                        prev_accent.clear();
                    }
                    if idx%8==4&&!accent_mute{
                        let avol=scale_mv(t_accent.volume.load(Ordering::Relaxed),mv);
                        prev_accent=m.clone();
                        for &note in &m{midi.note_on(ch_accent,note,((avol as u16*mv as u16)/127).min(127) as u8)}
                    }
                }

                midi.render_audio((delay_ms as f64 * 44.1) as usize);
                idx+=1;
            }
            if lc.stop.load(Ordering::Relaxed){break}
        }
        for n in &prev_nappe{midi.note_off(ch_str,*n)}
        for n in &prev_lead{midi.note_off(ch_lead,*n)}
        for n in &prev_accent{midi.note_off(ch_accent,*n)}
        if lc.stop.load(Ordering::Relaxed) || !do_loop {break}
    }
    midi.all_notes_off();
    midi.on_stop();
    println!("  done ({} evts)", ev.len());
}

// ─── Application des tracks ──────────────────────────────────────────────
fn apply_tracks(lc:&Live,cfg:&[TrackCfg]){
    for tc in cfg{
        let idx:Option<usize>=lc.tracks.iter().position(|t|t.channel==tc.channel);
        if let Some(i)=idx{
            if let Some(p)=tc.program{lc.tracks[i].program.store(p,Ordering::Relaxed)}
            if let Some(v)=tc.volume{lc.tracks[i].volume.store(v,Ordering::Relaxed)}
            if let Some(m)=tc.mute{lc.tracks[i].mute.store(m,Ordering::Relaxed)}
        }
    }
}

// ─── Routes ──────────────────────────────────────────────────────────────
async fn idx()->impl IntoResponse{Html(include_str!("../static/index.html"))}

async fn play(State(s):State<AppState>,Json(b):Json<PlayReq>)->impl IntoResponse{
    let lv=&s.live;
    if b.tempo>0{lv.tempo.store(b.tempo as u16,Ordering::Relaxed)}
    lv.sig.store(sig_code(&b.sig),Ordering::Relaxed);
    lv.pattern.store(pat(&b.pattern),Ordering::Relaxed);
    lv.stop.store(false,Ordering::Relaxed);
    if let Some(w)=b.walking{lv.walking.store(w,Ordering::Relaxed)}
    if let Some(ref t)=b.tracks{apply_tracks(lv,t)}
    if b.tracks.is_none() {
        lv.tracks[TRACK_LEAD].program.store(b.inst_val,Ordering::Relaxed);
        lv.tracks[TRACK_LEAD].mute.store(!b.arps,Ordering::Relaxed);
        lv.tracks[TRACK_BASS].mute.store(!b.bass,Ordering::Relaxed);
        lv.tracks[TRACK_STR].mute.store(!b.nappes,Ordering::Relaxed);
        lv.tracks[TRACK_DRUMS].mute.store(!b.drums,Ordering::Relaxed);
    }
    let do_loop=b.loop_enabled.unwrap_or(false);
    let ev:&[ChordEv]=if!b.seq.is_empty(){b.seq.as_slice()}else if!b.sequence.is_empty(){b.sequence.as_slice()}else{&[]};

    // Démarrer la loop sample si activée
    let tempo_now=lv.tempo.load(Ordering::Relaxed);
    let loop_active=lv.use_loops.load(Ordering::Relaxed);
    if loop_active {
        let lname=lv.loop_name.lock().unwrap().clone();
        let name_opt=if lname.is_empty(){None}else{Some(lname.as_str())};
        let lvol=lv.loop_volume.load(Ordering::Relaxed);
        samples::set_volume(lvol);
        samples::play_loop(tempo_now, name_opt, lv.loop_offset.load(Ordering::Relaxed));
    }

    // Cloner les backends pour le thread
    let synth_seq=s.synth.clone();
    let midi_seq=s.midi.as_ref().map(|h|Arc::clone(h));
    let synth_notes=s.synth.clone();
    let midi_notes=s.midi.as_ref().map(|h|Arc::clone(h));

    if!ev.is_empty(){
        let sq=ev.to_vec();let l=Arc::clone(lv);
        std::thread::spawn(move||{
            match (synth_seq, midi_seq) {
                (Some(s), Some(h)) => {
                    let mut r = s.lock().unwrap();
                    if let Ok(mut c) = h.lock() {
                        let mut multi = MultiOut { fluid: &mut *r, midi: &mut *c };
                        play_seq(&mut multi, &sq, &l, do_loop);
                    }
                }
                (Some(s), None) => {
                    let mut r = s.lock().unwrap();
                    play_seq(&mut *r, &sq, &l, do_loop);
                }
                (None, Some(h)) => {
                    if let Ok(mut c) = h.lock() {
                        play_seq(&mut *c, &sq, &l, do_loop);
                    }
                }
                (None, None) => {}
            }
        });
    }else if let Some(ref n)=b.notes{
        let v=n.clone();let l2=Arc::clone(lv);
        std::thread::spawn(move||{
            let mv=l2.master_vol.load(Ordering::Relaxed);
            match (synth_notes, midi_notes) {
                (Some(s), Some(h)) => {
                    let mut r = s.lock().unwrap();
                    if let Ok(mut c) = h.lock() {
                        let mut multi = MultiOut { fluid: &mut *r, midi: &mut *c };
                        play_notes(&mut multi, &v, mv);
                    }
                }
                (Some(s), None) => {
                    let mut r = s.lock().unwrap();
                    play_notes(&mut *r, &v, mv);
                }
                (None, Some(h)) => {
                    if let Ok(mut c) = h.lock() {
                        play_notes(&mut *c, &v, mv);
                    }
                }
                (None, None) => {}
            }
        });
    }
    Json(Rsp{status:"ok".into()})
}

async fn conf(State(s):State<AppState>,Json(b):Json<Cfg>)->impl IntoResponse{
    let lv=&s.live;
    if let Some(ref t)=b.tracks{apply_tracks(lv,t)}
    if let Some(v)=b.drums{lv.tracks[TRACK_DRUMS].mute.store(!v,Ordering::Relaxed)}
    if let Some(v)=b.bass{lv.tracks[TRACK_BASS].mute.store(!v,Ordering::Relaxed)}
    if let Some(v)=b.arpeggios{lv.tracks[TRACK_LEAD].mute.store(!v,Ordering::Relaxed)}
    if let Some(v)=b.nappes{lv.tracks[TRACK_STR].mute.store(!v,Ordering::Relaxed)}
    if let Some(ref p)=b.pattern{lv.pattern.store(pat(p),Ordering::Relaxed)}
    if let Some(t)=b.tempo{lv.tempo.store(t,Ordering::Relaxed);samples::set_current_tempo(t);}
    if let Some(ref sg)=b.sig{lv.sig.store(sig_code(sg),Ordering::Relaxed)}
    if let Some(iv)=b.instrument{lv.tracks[TRACK_LEAD].program.store(iv,Ordering::Relaxed)}
    if let Some(w)=b.walking{lv.walking.store(w,Ordering::Relaxed)}
    if let Some(m)=b.master_vol{lv.master_vol.store(m,Ordering::Relaxed);}
    if let Some(u)=b.use432{
        let was=lv.use432.swap(u,Ordering::Relaxed);
        if was!=u{
            if let Some(ref h)=s.midi{
                if let Ok(mut c)=h.lock(){
                    for&ch in&[0u8,2,3,4]{c.pitch_bend(ch,if u{6881}else{8192})}
                }
            }
        }
    }
    if let Some(off)=b.loop_offset{lv.loop_offset.store(off,Ordering::Relaxed);samples::update_offset(off);}
    if let Some(lo)=b.use_loops{lv.use_loops.store(lo,Ordering::Relaxed);samples::set_use_loops(lo);}
    if let Some(ref n)=b.loop_name{*lv.loop_name.lock().unwrap()=n.clone();}
    if let Some(lv2)=b.loop_volume{lv.loop_volume.store(lv2,Ordering::Relaxed);samples::set_volume(lv2);}
    Json(Rsp{status:"ok".into()})
}

async fn stop(State(s):State<AppState>)->impl IntoResponse{
    s.live.stop.store(true,Ordering::Relaxed);
    samples::stop_loop();
    if let Some(ref h)=s.midi{if let Ok(mut c)=h.lock(){c.all_notes_off()}}
    Json(serde_json::json!({"status":"stopped"}))
}

// ─── WebSocket Audio Stream ─────────────────────────────────────────────
fn read_audio_chunk(buf: &AudioBuffer, chunk_frames: usize, max_frames: usize) -> Option<Vec<f32>> {
    let mut guard = buf.lock().ok()?;
    let avail = guard.len();
    let stereo_frames = avail / 2;
    if stereo_frames == 0 { return None; }
    if stereo_frames > max_frames {
        let to_skip = (stereo_frames - max_frames / 4) * 2;
        guard.drain(..to_skip);
    }
    let take = (chunk_frames * 2).min(guard.len());
    let samples: Vec<f32> = guard.drain(..take).collect();
    Some(samples)
}

async fn handle_audio_stream(mut socket: WebSocket, state: AppState) {
    println!("   📡 WebSocket audio connecté");
    let buf = match &state.audio_buffer {
        Some(b) => b.clone(),
        None => { let _ = socket.send(Message::Text("NO_SYNTH".into())).await; return; }
    };
    const CHUNK_FRAMES: usize = 2048;
    const MAX_BUFFER_FRAMES: usize = 88200;
    loop {
        let samples = read_audio_chunk(&buf, CHUNK_FRAMES, MAX_BUFFER_FRAMES);
        let chunk = match samples { Some(s) if !s.is_empty() => s, _ => {
            tokio::time::sleep(Duration::from_millis(20)).await; continue;
        }};
        let bytes: Vec<u8> = chunk.iter().flat_map(|&s| s.to_ne_bytes().to_vec()).collect();
        if let Err(e) = socket.send(Message::Binary(bytes.into())).await {
            println!("   📡 WebSocket déconnecté : {e:?}"); break;
        }
        let chunk_dur_ms = (chunk.len() as f64 / 2.0 / 44.1) as u64;
        tokio::time::sleep(Duration::from_millis(chunk_dur_ms.min(80).max(5))).await;
    }
    println!("   📡 WebSocket audio déconnecté");
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_audio_stream(socket, state))
}

async fn render_wav(Json(b): Json<PlayReq>) -> impl IntoResponse {
    use axum::http::{HeaderMap, StatusCode};
    let ev: &[ChordEv] = if !b.seq.is_empty() { &b.seq } else if !b.sequence.is_empty() { &b.sequence } else { &[] };
    if ev.is_empty() { return (StatusCode::BAD_REQUEST, "Empty sequence").into_response(); }
    let mut notes_arrays: Vec<Vec<u8>> = Vec::new();
    let mut beats: Vec<f64> = Vec::new();
    for e in ev { notes_arrays.push(notes_from_ev(e)); beats.push(e.beats); }
    let smf = render::generate_smf(&notes_arrays, &beats, b.tempo, 1);
    let sf_path = "/usr/share/sounds/sf3/MuseScore_General_Full.sf3";
    match render::render_wav(&smf, sf_path) {
        Ok(wav) => { let mut h = HeaderMap::new(); h.insert("Content-Type", "audio/wav".parse().unwrap()); (StatusCode::OK, h, wav).into_response() }
        Err(e) => { (StatusCode::INTERNAL_SERVER_ERROR, e).into_response() }
    }
}

#[tokio::main]
async fn main(){
    println!("csrs");let midi=init_midi();
    samples::init();

    let mut audio_buffer: Option<AudioBuffer> = None;
    let sf_paths = ["/usr/share/sounds/sf3/MuseScore_General_Full.sf3","/usr/share/sounds/sf2/FluidR3_GM.sf2","/usr/share/sounds/sf2/TimGM6mb.sf2"];
    let synth = sf_paths.iter().find_map(|path| {
        match SynthRenderer::new(path, 44100) {
            Ok((s, buf)) => { audio_buffer = Some(buf); println!("   ✅ Synthé FluidSynth : {path}"); Some(Arc::new(Mutex::new(s))) }
            Err(e) => { eprintln!("   ⚠️  Impossible de charger {path} : {e}"); None }
        }
    });

    let state=AppState{midi,synth,audio_buffer,live:Arc::new(Live{
        tracks: [LiveTrack::new(0,51,15),LiveTrack::new(2,33,40),LiveTrack::new(3,48,30),LiveTrack::new(9,1,80),LiveTrack::new(4,2,20)],
        pattern:AtomicU8::new(PAT_ROCK),tempo:AtomicU16::new(120),stop:AtomicBool::new(false),sig:AtomicU16::new(44),
        walking:AtomicBool::new(false),master_vol:AtomicU8::new(127),use432:AtomicBool::new(false),loop_offset:AtomicI32::new(0),use_loops:AtomicBool::new(false),
        loop_name:Mutex::new(String::new()),loop_volume:AtomicU8::new(80),
    })};
    async fn samples_list()->impl IntoResponse{
    use axum::http::StatusCode;
    let data=samples::get_available();
    (StatusCode::OK,axum::Json(data))
}

let app=Router::new().route("/",get(idx)).route("/play",post(play))
        .route("/config",post(conf)).route("/stop",post(stop))
        .route("/render-wav",post(render_wav))
        .route("/samples-list",get(samples_list))
        .route("/audio-stream",get(ws_handler))
        .layer(CorsLayer::permissive()).with_state(state);
    let p=std::env::var("PORT").unwrap_or_else(|_|"4000".to_string());
    println!("http://0.0.0.0:{p}");
    let l=tokio::net::TcpListener::bind(format!("0.0.0.0:{p}")).await.unwrap();
    axum::serve(l,app).await.unwrap();
}
