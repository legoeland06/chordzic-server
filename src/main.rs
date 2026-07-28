mod render;
mod samples;
use axum::{extract::State, response::{Html, Json, IntoResponse}, routing::{get, post}, Router};
use midir::{MidiOutput, MidiOutputConnection};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU16, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower_http::cors::CorsLayer;

type MidiHandle = Arc<Mutex<MidiOutputConnection>>;

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

#[derive(Clone)] struct AppState{midi:Option<MidiHandle>,live:Arc<Live>}

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
    // midi[0] = basse, midi[1] = fondamentale, midi[2+] = notes de l'accord
    if midi.len() < 3 { return false; }
    let root = midi[1];
    // Chercher une tierce (3 ou 4 demi-tons au-dessus de la fondamentale)
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
    // Mineur si on a une tierce mineure et pas de tierce majeure
    has_minor && !has_major
}

fn init_midi()->Option<MidiHandle>{
    let mo=MidiOutput::new("cs").ok()?;let p=mo.ports();
    if p.is_empty(){eprintln!("no port");return None}
    println!("Ports:");for(i,x)in p.iter().enumerate(){if let Ok(n)=mo.port_name(x){println!(" [{i}] {n}")}}
    let i:usize=if let Ok(e)=std::env::var("MIDI_PORT"){e.parse().unwrap_or(2)}else{2};
    if i>=p.len(){eprintln!("port {i} invalid");return None}
    println!("Connecte {}",mo.port_name(&p[i]).unwrap_or_default());
    mo.connect(&p[i],"cs").ok().map(|c|Arc::new(Mutex::new(c)))
}

fn snd(c:&mut MidiOutputConnection,m:&[u8]){if let Err(e)=c.send(m){eprintln!("⚠️{e}")}}
fn cc(c:&mut MidiOutputConnection,ch:u8,ctl:u8,v:u8){snd(c,&[0xB0|ch,ctl,v])}
fn pc(c:&mut MidiOutputConnection,ch:u8,v:u8){snd(c,&[0xC0|ch,v])}
fn no(c:&mut MidiOutputConnection,ch:u8,n:u8,v:u8){snd(c,&[0x90|ch,n,v])}
fn no_mv(c:&mut MidiOutputConnection,ch:u8,n:u8,v:u8,mv:u8){
    snd(c,&[0x90|ch,n,((v as u16*mv as u16)/127).min(127)as u8])}
fn rch(c:&mut MidiOutputConnection){for&ch in&[0u8,2,3,4,9]{cc(c,ch,123,0)}}
fn pb(c:&mut MidiOutputConnection,ch:u8,val:u16){let lsb=(val&127)as u8;let msb=((val>>7)&127)as u8;snd(c,&[0xE0|ch,lsb,msb])}



// ─── Walking Bass ────────────────────────────────────────────────────────
/// Maintient une note dans la tessiture basse (MIN_NOTE-MAX_NOTE)
fn bass_clamp(n: u8) -> u8 {
    if n < MIN_NOTE { n + 12 }
    else if n > MAX_NOTE { n - 12 }
    else { n }
}

/// Genere 4 notes de walking bass pour une mesure (4 temps)
/// current_notes: [root, chord_tone1, chord_tone2, ...] en MIDI absolu
/// next_root: fondamentale du prochain accord en MIDI absolu
/// minor: si vrai, temps 2 = fondamentale + 2 demi-tons (ton au-dessus)
fn generate_walking_bass(current_notes: &[u8], next_root: u8, seed: u64, minor: bool) -> [u8; 4] {
    let root = current_notes[0];
    let chord_tones: Vec<u8> = if current_notes.len() > 1 {
        // Ramener les chord tones dans l'octave basse (MIDI MIN_NOTE-MAX_NOTE)
        current_notes[1..].iter().map(|&n| bass_clamp(n)).collect()
    } else {
        vec![root - 5] // quinte par defaut
    };
    // Enlever les doublons
    let mut tones: Vec<u8> = chord_tones.clone();
    tones.sort();
    tones.dedup();
    let tones = tones;

    // Temps 1 : fondamentale (ancrage)
    let b1 = root;

    // Temps 2 : si mineur, 50% ton au-dessus de la fondamentale, 50% chord tone aleatoire
    let b2 = if minor {
        match seed % 100 {
            0..=24 => root + 2,
            25..=49 => root - 10,
            _ => {
                let idx2 = (seed as usize) % tones.len();
                tones[idx2]
            }
        }
    } else {
        let idx2 = (seed as usize) % tones.len();
        tones[idx2]
    };

    // Temps 3 : chord tone different du temps 2
    let filtered: Vec<u8> = tones.iter().filter(|&&n| n != b2).copied().collect();
    let b3 = if filtered.is_empty() { b2 + 7 } else { filtered[(seed.wrapping_add(7) as usize) % filtered.len()] };

    // Temps 4 : note d'approche vers next_root
    let b4 = match (seed % 100) as u8 {
        0..=49 => { // Approche chromatique (50%)
            let dir = if seed % 2 == 0 { 1i8 } else { -1i8 };
            let mut app = (next_root as i16 + dir as i16) as u8;
            // Si l'approche est trop loin de la tessiture, essayer l'autre direction
            if app < MIN_NOTE || app > MAX_NOTE {
                app = (next_root as i16 - dir as i16) as u8;
            }
            // Dernier recours : next_root lui-meme
            if app < MIN_NOTE { app = next_root + 12; }
            if app > MAX_NOTE { app = next_root - 12; }
            app
        }
        50..=67 => { // Approche dominante (35%)
            let mut app = next_root + 7;
            if app > MAX_NOTE { app -= 12; }
            app
        }
        68..=85 => { // Approche sous-dominante (15%)
            let mut app = next_root - 5;
            if app < MIN_NOTE { app += 12; }
            app
        }
        _ => { // Approche diatonique (15%) : chord tone le plus proche de next_root
            tones.iter()
                .min_by_key(|&&t| {
                    let diff = if t > next_root { t - next_root } else { next_root - t };
                    diff
                })
                .copied()
                .unwrap_or(next_root)
        }
    };

    [b1, b2, b3, b4]
}



// ─── MIDI helpers ───────────────────────────────────────────────────────
const DRUM_KICK:u8=36; const DRUM_SNARE:u8=38; const DRUM_RIM:u8=37;
const DRUM_HH:u8=42; const DRUM_RIDE:u8=51;
const HH_BEAT:u8=80; const HH_8TH:u8=65;
fn scale_mv(v:u8,mv:u8)->u8{((v as u16*mv as u16)/127).min(127)as u8}

fn drum_hit(c:&mut MidiOutputConnection,beat:u64,pat:u8,on_beat:bool,on_eighth:bool,bars:u64,vol:u8,mv:u8){
    if!on_beat&&!on_eighth{return}
    let b=beat%bars;
    let v=scale_mv(vol,mv);
    let hh=vscale(v,HH_BEAT);let h8=vscale(v,HH_8TH);let h55=vscale(v,55);let h45=vscale(v,45);let h40=vscale(v,10);
    let h60=vscale(v,60);let h65=vscale(v,65);
    match pat{
        PAT_REGGAE=>if on_beat{match b{
            0=>{no(c,9,DRUM_HH,h60);}
            1=>{no(c,9,DRUM_HH,h60);}
            2=>{no(c,9,DRUM_KICK,vscale(v,120));no(c,9,DRUM_HH,h65);no(c,9,DRUM_RIM,vscale(v,90));}
            3=>{no(c,9,DRUM_HH,h60);}_=>{}
        }}else if on_eighth{no(c,9,DRUM_HH,h40);}
        PAT_JAZZ=>{
            let b=beat%8; // 2 mesures
            if on_beat{match b{
                0=>{no(c,9,DRUM_RIDE,h60);}
                2=>{no(c,9,DRUM_RIDE,h60);}
                4=>{no(c,9,DRUM_RIDE,h60);no(c,9,44,vscale(v,40));} // ride + pedal HH
                6=>{no(c,9,DRUM_RIDE,h60);}
                7=>{no(c,9,DRUM_RIDE,h60);no(c,9,44,vscale(v,40));no(c,9,DRUM_RIM,vscale(v,50));}_=>{}
            }}else if on_eighth{no(c,9,DRUM_HH,35);}
        }
        PAT_POP=>{
            let b=beat%8; // 2 mesures
            if on_beat{match b{
            0=>{no(c,9,DRUM_KICK,vscale(v,85));no(c,9,DRUM_HH,vscale(v,50));}
            2=>{no(c,9,DRUM_SNARE,vscale(v,70));no(c,9,DRUM_HH,vscale(v,50));}
            4=>{no(c,9,DRUM_KICK,vscale(v,75));no(c,9,DRUM_HH,vscale(v,50));}
            6=>{no(c,9,DRUM_SNARE,vscale(v,65));no(c,9,DRUM_HH,vscale(v,50));}_=>{}
        }}else if on_eighth{no(c,9,DRUM_HH,vscale(v,45));}
    }
        PAT_BOSSA=>if on_beat{match b{
            0=>{no(c,9,DRUM_KICK,vscale(v,55));no(c,9,DRUM_HH,h45);}1=>{no(c,9,DRUM_SNARE,vscale(v,30));no(c,9,DRUM_HH,h45);}
            2=>{no(c,9,DRUM_KICK,vscale(v,60));no(c,9,DRUM_HH,h45);}3=>{no(c,9,DRUM_KICK,vscale(v,50));no(c,9,DRUM_HH,h45);}_=>{}
        }}else if on_eighth{no(c,9,DRUM_HH,h40);}
        PAT_ONEDROP=>if on_beat{match b{
            0=>{no(c,9,DRUM_KICK,vscale(v,90));no(c,9,DRUM_HH,h55);}
            1=>{no(c,9,DRUM_HH,h40);}
            2=>{no(c,9,DRUM_KICK,vscale(v,90));no(c,9,DRUM_RIM,vscale(v,65));no(c,9,DRUM_HH,h45);}
            3=>{no(c,9,DRUM_HH,h55);}_=>{}
        }}else if on_eighth{no(c,9,DRUM_HH,h40);}
        _=>if on_beat{match b{
            0=>{no(c,9,DRUM_KICK,vscale(v,90));no(c,9,DRUM_HH,hh);}1=>{no(c,9,DRUM_SNARE,vscale(v,75));no(c,9,DRUM_HH,hh);}
            2=>{no(c,9,DRUM_KICK,vscale(v,80));no(c,9,DRUM_HH,hh);}3=>{no(c,9,DRUM_SNARE,vscale(v,70));no(c,9,DRUM_HH,hh);}_=>{}
        }}else if on_eighth{no(c,9,DRUM_HH,h8);}
    }
}
fn vscale(vol:u8,base:u8)->u8{((vol as u16*base as u16)/127).min(127) as u8}

fn play_notes(c:&mut MidiOutputConnection,notes:&[String],mv:u8){
    let mut v:Vec<u8>=vec![];for n in notes{if let Ok(m)=note_midi(n){v.push(m)}}if v.is_empty(){return}
    rch(c);
    for&ch in&[0u8,2,3]{cc(c,ch,101,0);cc(c,ch,100,1);cc(c,ch,6,62);cc(c,ch,38,2)}
    pc(c,0,51);pc(c,2,33);
    for _ in 0..2{for&n in&v{std::thread::sleep(Duration::from_millis(240));if n<MIN_NOTE{no_mv(c,2,n,35,mv)}else{no_mv(c,0,n,15,mv)}}}
    rch(c);println!("  notes: {v:?}");
}

fn setup_tracks(c:&mut MidiOutputConnection,lc:&Live){
    for t in &lc.tracks {
        let ch=t.channel;
        if ch==9{pc(c,ch,1);continue}
        pc(c,ch,t.program.load(Ordering::Relaxed)as u8);
    }
}

fn play_seq(c:&mut MidiOutputConnection,ev:&[ChordEv],lc:&Live,do_loop:bool){
    loop {
        rch(c);
        setup_tracks(c,lc);
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
        let _loop_on=lc.use_loops.load(Ordering::Relaxed);
        let _l_off=lc.loop_offset.load(Ordering::Relaxed);
        let mut seed:u64 = 0;

        for(i,e)in ev.iter().enumerate(){
            let mut m:Vec<u8>=vec![];for n in&e.notes{if let Ok(x)=note_midi(n){m.push(x)}}
            if m.is_empty(){
                // Silence : seule la batterie continue
                if i>0{rch(c);cc(c,9,120,0)}
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
                    // Batterie seulement
                    if !t_drums.mute.load(Ordering::Relaxed){
                        if last_b_drums==u64::MAX||beat>last_b_drums{
                            let dvol=scale_mv(t_drums.volume.load(Ordering::Relaxed),mv);
                            drum_hit(c,beat,pt,true,false,bars,dvol,127);
                            last_b_drums=beat;
                        }
                        let beat_pos=elapsed_ms%bd_ms;
                        if beat_pos>bd_ms/2.0-10.0&&beat_pos<bd_ms/2.0+10.0{
                            let dvol=scale_mv(t_drums.volume.load(Ordering::Relaxed),mv);
                            drum_hit(c,beat,pt,false,true,bars,dvol,127);
                        }
                    }
                    idx+=1;
                }
                if lc.stop.load(Ordering::Relaxed){break}
                continue;
            }
            if i>0{rch(c);cc(c,9,120,0)}

            let root=m[0];
            let nappe_notes:Vec<u8>=m[1..].to_vec();
            let dur=(60_000.0/lc.tempo.load(Ordering::Relaxed).max(20)as f64*e.beats)as u64;
            let start=std::time::Instant::now();
            let mut idx=0u64;
            let mut last_b_drums=u64::MAX;
            let mut last_b_bass=0u64;  // 0 = deja joue manuellement
            let mut prev_bass_note:u8=0;

            // Generation du walking bass pour cet accord
            let mut walking_notes:[u8;4]=[root,root,root,root];
            if walking && PAT_REGGAE != lc.pattern.load(Ordering::Relaxed) {
                let next_root = if let Some(ne) = ev.get(i+1) {
                    let nv = notes_from_ev(ne);
                    if !nv.is_empty() { nv[0] } else { root }
                } else {
                    // Dernier accord : boucler sur le premier
                    if let Some(ne) = ev.get(0) {
                        let nv = notes_from_ev(ne);
                        if !nv.is_empty() { nv[0] } else { root }
                    } else { root }
                };
                walking_notes = generate_walking_bass(&m, next_root, seed, is_minor(e));
                seed = seed.wrapping_add(1);
            }

            // Basse : jouer le premier temps IMMEDIATEMENT (pas via le tick)
            if !t_bass.mute.load(Ordering::Relaxed) {
                let bvol=scale_mv(t_bass.volume.load(Ordering::Relaxed),mv);
                let bass_note = if walking { walking_notes[0] } else { root };
                no_mv(c,ch_bass,bass_note,bvol,mv);
                prev_bass_note=bass_note;
                last_b_bass=0;
            }

            // Nappes (Strings)
            if !t_str.mute.load(Ordering::Relaxed) {
                for n in &prev_nappe{no(c,ch_str,*n,0)}
                let str_vol=scale_mv(t_str.volume.load(Ordering::Relaxed),mv);
                for n in &nappe_notes{no_mv(c,ch_str,*n,str_vol,mv)}
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

                // Batterie
                if !t_drums.mute.load(Ordering::Relaxed){
                    if last_b_drums==u64::MAX||beat>last_b_drums{
                        let dvol=scale_mv(t_drums.volume.load(Ordering::Relaxed),mv);
                        drum_hit(c,beat,pt,true,false,bars,dvol,127);
                        last_b_drums=beat;
                    }
                    let beat_pos=elapsed_ms%bd_ms;
                    if beat_pos>bd_ms/2.0-10.0&&beat_pos<bd_ms/2.0+10.0{
                        let dvol=scale_mv(t_drums.volume.load(Ordering::Relaxed),mv);
                        drum_hit(c,beat,pt,false,true,bars,dvol,127);
                    }
                }

                // Basse (Walking ou root)
                if !t_bass.mute.load(Ordering::Relaxed){
                    if beat>last_b_bass{
                        let bvol=scale_mv(t_bass.volume.load(Ordering::Relaxed),mv);
                        let bass_note = if walking {
                            let bi = (beat % 4) as usize;
                            walking_notes[bi]
                        } else {
                            root
                        };
                        no(c,ch_bass,prev_bass_note,0);
                        no_mv(c,ch_bass,bass_note,bvol,mv);
                        prev_bass_note=bass_note;
                        last_b_bass=beat;
                    }
                }

                // Pompe skank (Lead) - staccato sur contretemps (8eme)
                if !m.is_empty(){
                    let lead_mute=t_lead.mute.load(Ordering::Relaxed);
                    // Note Off au tick suivant immediat (staccato)
                    if idx%4==3&&!prev_lead.is_empty(){
                        for &n in &prev_lead{no(c,ch_lead,n,0)}
                        prev_lead.clear();
                    }
                    // Note On sur le contretemps 8eme (3e 16eme du temps)
                    if idx%4==2&&!lead_mute{
                        let lvol=scale_mv(t_lead.volume.load(Ordering::Relaxed),mv);
                        prev_lead=m.clone();
                        for &note in &m{no_mv(c,ch_lead,note,lvol,mv)}
                    }
                }

                // Pompe accent temps 2&4 (canal 4, piano sec)
                if !m.is_empty(){
                    let accent_mute=t_accent.mute.load(Ordering::Relaxed);
                    // Note Off au tick apres le temps 2 ou 4
                    if idx%8==5&&!prev_accent.is_empty(){
                        for &n in &prev_accent{no(c,ch_accent,n,0)}
                        prev_accent.clear();
                    }
                    // Note On sur temps 2 et 4 (tick 4 et 12 = idx%8==4)
                    if idx%8==4&&!accent_mute{
                        let avol=scale_mv(t_accent.volume.load(Ordering::Relaxed),mv);
                        prev_accent=m.clone();
                        for &note in &m{no_mv(c,ch_accent,note,avol,mv)}
                    }
                }

                idx+=1;
            }
            if lc.stop.load(Ordering::Relaxed){break}
        }
        for n in &prev_nappe{no(c,ch_str,*n,0)}
        for n in &prev_lead{no(c,ch_lead,*n,0)}
        for n in &prev_accent{no(c,ch_accent,*n,0)}
        if lc.stop.load(Ordering::Relaxed) || !do_loop {break}
    }
    rch(c);
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
    if let Some(ref h)=s.midi{
        let h2=Arc::clone(h);
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
        if!ev.is_empty(){let sq=ev.to_vec();let l=Arc::clone(lv);
            std::thread::spawn(move||{if let Ok(mut c)=h2.lock(){play_seq(&mut c,&sq,&l,do_loop)}});
        }else if let Some(ref n)=b.notes{let v=n.clone();let l2=Arc::clone(lv);
            std::thread::spawn(move||{let mv=l2.master_vol.load(Ordering::Relaxed);if let Ok(mut c)=h2.lock(){play_notes(&mut c,&v,mv)}});
        }
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
                    for&ch in&[0u8,2,3,4]{pb(&mut c,ch,if u{6881}else{8192})}
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
    if let Some(ref h)=s.midi{if let Ok(mut c)=h.lock(){rch(&mut c)}}
    Json(serde_json::json!({"status":"stopped"}))
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

    // Construire la config de rendu
    let mut tracks_cfg: [render::TrackCfg; 5] = [
        render::TrackCfg { channel: 0, program: b.inst_val, volume: 15, mute: !b.arps },
        render::TrackCfg { channel: 2, program: 33, volume: 40, mute: !b.bass },
        render::TrackCfg { channel: 3, program: 48, volume: 30, mute: !b.nappes },
        render::TrackCfg { channel: 9, program: 1, volume: 80, mute: !b.drums },
        render::TrackCfg { channel: 4, program: 2, volume: 20, mute: false },
    ];
    // Appliquer tracks override si présent
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
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, e).into_response()
        }
    }
}

#[tokio::main]
async fn main(){
    println!("csrs");let midi=init_midi();
    samples::init();
    let state=AppState{midi,live:Arc::new(Live{
        tracks: [
            LiveTrack::new(0,51,15),   // Lead (canal 0)
            LiveTrack::new(2,33,40),   // Bass (canal 2)
            LiveTrack::new(3,48,30),   // Strings (canal 3)
            LiveTrack::new(9,1,80),    // Drums (canal 9)
            LiveTrack::new(4,2,20),    // Accent (canal 4, Bright Acoustic Piano)
        ],
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
        .layer(CorsLayer::permissive()).with_state(state);
    let p=std::env::var("PORT").unwrap_or_else(|_|"4000".to_string());
    println!("http://0.0.0.0:{p}");
    let l=tokio::net::TcpListener::bind(format!("0.0.0.0:{p}")).await.unwrap();
    axum::serve(l,app).await.unwrap();
}
