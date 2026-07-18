use axum::{extract::State, response::{Html, Json, IntoResponse}, routing::{get, post}, Router};
use midir::{MidiOutput, MidiOutputConnection};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower_http::cors::CorsLayer;

type MidiHandle = Arc<Mutex<MidiOutputConnection>>;

// ─── Tracks ──────────────────────────────────────────────────────────────
const TRACK_LEAD:usize=0;
const TRACK_BASS:usize=1;
const TRACK_STR:usize=2;
const TRACK_DRUMS:usize=3;

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
    tracks: [LiveTrack; 4],
    pattern: AtomicU8,
    tempo: AtomicU16,
    stop: AtomicBool,
    sig: AtomicU16,
    walking: AtomicBool,
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
fn rch(c:&mut MidiOutputConnection){for&ch in&[0u8,2,3,9]{cc(c,ch,123,0)}}

// ─── Walking Bass ────────────────────────────────────────────────────────
/// Genere 4 notes de walking bass pour une mesure (4 temps)
/// current_notes: [root, chord_tone1, chord_tone2, ...] en MIDI absolu
/// next_root: fondamentale du prochain accord en MIDI absolu
fn generate_walking_bass(current_notes: &[u8], next_root: u8, seed: u64) -> [u8; 4] {
    let root = current_notes[0];
    let chord_tones: Vec<u8> = if current_notes.len() > 1 {
        // Ramener les chord tones dans l'octave basse (MIDI 28-48)
        current_notes[1..].iter().map(|&n| {
            let mut oct = n;
            while oct > root + 12 && oct >= 12 { oct -= 12; }
            while oct < root - 12 { oct += 12; }
            oct
        }).collect()
    } else {
        vec![root + 7] // quinte par defaut
    };
    // Enlever les doublons
    let mut tones: Vec<u8> = chord_tones.clone();
    tones.sort();
    tones.dedup();
    let tones = tones;

    // Temps 1 : fondamentale (ancrage)
    let b1 = root;

    // Temps 2 : chord tone aleatoire
    let idx2 = (seed as usize) % tones.len();
    let b2 = tones[idx2];

    // Temps 3 : chord tone different du temps 2
    let filtered: Vec<u8> = tones.iter().filter(|&&n| n != b2).copied().collect();
    let b3 = if filtered.is_empty() { b2 + 7 } else { filtered[(seed.wrapping_add(7) as usize) % filtered.len()] };

    // Temps 4 : note d'approche vers next_root
    let b4 = match (seed % 100) as u8 {
        0..=49 => { // Approche chromatique (50%)
            let dir = if seed % 2 == 0 { 1i8 } else { -1i8 };
            let mut app = (next_root as i16 + dir as i16) as u8;
            // Si l'approche est trop loin de la tessiture, essayer l'autre direction
            if app < 28 || app > 48 {
                app = (next_root as i16 - dir as i16) as u8;
            }
            // Dernier recours : next_root lui-meme
            if app < 28 { app = next_root + 12; }
            if app > 48 { app = next_root - 12; }
            app
        }
        50..=84 => { // Approche dominante (35%)
            let mut app = next_root + 7;
            if app > 48 { app -= 12; }
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

/// Maintient une note dans la tessiture basse (28-48)
fn bass_clamp(n: u8) -> u8 {
    if n < 28 { n + 12 }
    else if n > 48 { n - 12 }
    else { n }
}

// ─── MIDI helpers ───────────────────────────────────────────────────────
const DRUM_KICK:u8=36; const DRUM_SNARE:u8=38; const DRUM_RIM:u8=37;
const DRUM_HH:u8=42; const DRUM_RIDE:u8=51;
const HH_BEAT:u8=80; const HH_8TH:u8=65;

fn drum_hit(c:&mut MidiOutputConnection,beat:u64,pat:u8,on_beat:bool,on_eighth:bool,bars:u64,vol:u8){
    if!on_beat&&!on_eighth{return}
    let b=beat%bars;
    let hh=vscale(vol,HH_BEAT);let h8=vscale(vol,HH_8TH);let h55=vscale(vol,55);let h50=vscale(vol,50);let h45=vscale(vol,45);let h40=vscale(vol,40);
    let h60=vscale(vol,60);let h65=vscale(vol,65);
    match pat{
        PAT_REGGAE=>if on_beat{match b{
            0=>{no(c,9,DRUM_HH,h60);}1=>{no(c,9,DRUM_RIM,vscale(vol,70));no(c,9,DRUM_HH,h60);}
            2=>{no(c,9,DRUM_KICK,vscale(vol,85));no(c,9,DRUM_HH,h65);}3=>{no(c,9,DRUM_RIM,vscale(vol,70));no(c,9,DRUM_HH,h60);}_=>{}
        }}else if on_eighth{no(c,9,DRUM_HH,h55);}
        PAT_JAZZ=>{
            let b=beat%8; // 2 mesures
            if on_beat{match b{
                0=>{no(c,9,DRUM_RIDE,h60);}
                2=>{no(c,9,DRUM_RIDE,h60);}
                4=>{no(c,9,DRUM_RIDE,h60);no(c,9,44,vscale(vol,40));} // ride + pedal HH
                6=>{no(c,9,DRUM_RIDE,h60);}
                7=>{no(c,9,DRUM_RIDE,h60);no(c,9,44,vscale(vol,40));no(c,9,DRUM_RIM,vscale(vol,50));}_=>{}
            }}else if on_eighth{no(c,9,DRUM_HH,35);}
        }
        PAT_POP=>if on_beat{match b{
            0=>{no(c,9,DRUM_KICK,vscale(vol,85));no(c,9,DRUM_HH,vscale(vol,50));}1=>{no(c,9,DRUM_SNARE,vscale(vol,70));no(c,9,DRUM_HH,vscale(vol,50));}
            2=>{no(c,9,DRUM_KICK,vscale(vol,75));no(c,9,DRUM_HH,vscale(vol,50));}3=>{no(c,9,DRUM_SNARE,vscale(vol,65));no(c,9,DRUM_HH,vscale(vol,50));}_=>{}
        }}else if on_eighth{no(c,9,DRUM_HH,vscale(vol,45));}
        PAT_BOSSA=>if on_beat{match b{
            0=>{no(c,9,DRUM_KICK,vscale(vol,55));no(c,9,DRUM_HH,h45);}1=>{no(c,9,DRUM_SNARE,vscale(vol,30));no(c,9,DRUM_HH,h45);}
            2=>{no(c,9,DRUM_KICK,vscale(vol,60));no(c,9,DRUM_HH,h45);}3=>{no(c,9,DRUM_KICK,vscale(vol,50));no(c,9,DRUM_HH,h45);}_=>{}
        }}else if on_eighth{no(c,9,DRUM_HH,h40);}
        PAT_ONEDROP=>if on_beat{match b{
            0=>{no(c,9,DRUM_HH,h55);}1=>{no(c,9,DRUM_HH,h55);}
            2=>{no(c,9,DRUM_KICK,vscale(vol,90));no(c,9,DRUM_RIM,vscale(vol,65));no(c,9,DRUM_HH,h60);}3=>{no(c,9,DRUM_HH,h55);}_=>{}
        }}else if on_eighth{no(c,9,DRUM_HH,h50);}
        _=>if on_beat{match b{
            0=>{no(c,9,DRUM_KICK,vscale(vol,90));no(c,9,DRUM_HH,hh);}1=>{no(c,9,DRUM_SNARE,vscale(vol,75));no(c,9,DRUM_HH,hh);}
            2=>{no(c,9,DRUM_KICK,vscale(vol,80));no(c,9,DRUM_HH,hh);}3=>{no(c,9,DRUM_SNARE,vscale(vol,70));no(c,9,DRUM_HH,hh);}_=>{}
        }}else if on_eighth{no(c,9,DRUM_HH,h8);}
    }
}
fn vscale(vol:u8,base:u8)->u8{((vol as u16*base as u16)/127).min(127) as u8}

fn play_notes(c:&mut MidiOutputConnection,notes:&[String]){
    let mut v:Vec<u8>=vec![];for n in notes{if let Ok(m)=note_midi(n){v.push(m)}}if v.is_empty(){return}
    rch(c);
    for&ch in&[0u8,2,3]{cc(c,ch,101,0);cc(c,ch,100,1);cc(c,ch,6,62);cc(c,ch,38,2)}
    pc(c,0,51);pc(c,2,33);
    for _ in 0..2{for&n in&v{std::thread::sleep(Duration::from_millis(240));if n<48{no(c,2,n,35)}else{no(c,0,n,15)}}}
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
        let t_lead=&lc.tracks[TRACK_LEAD];
        let t_bass=&lc.tracks[TRACK_BASS];
        let t_str=&lc.tracks[TRACK_STR];
        let t_drums=&lc.tracks[TRACK_DRUMS];
        let ch_lead=t_lead.channel;
        let ch_bass=t_bass.channel;
        let ch_str=t_str.channel;
        let walking=lc.walking.load(Ordering::Relaxed);
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
                while(start.elapsed().as_millis()as u64)<dur&&!lc.stop.load(Ordering::Relaxed){
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
                            let dvol=t_drums.volume.load(Ordering::Relaxed);
                            drum_hit(c,beat,pt,true,false,bars,dvol);
                            last_b_drums=beat;
                        }
                        let beat_pos=elapsed_ms%bd_ms;
                        if beat_pos>bd_ms/2.0-10.0&&beat_pos<bd_ms/2.0+10.0{
                            let dvol=t_drums.volume.load(Ordering::Relaxed);
                            drum_hit(c,beat,pt,false,true,bars,dvol);
                        }
                    }
                    idx+=1;
                }
                if lc.stop.load(Ordering::Relaxed){break}
                continue;
            }
            if i>0{rch(c);cc(c,9,120,0)}

            let root=m[0];let arp:Vec<u8>=m[1..].to_vec();
            let nappe_notes:Vec<u8>=m[1..].to_vec();
            let dur=(60_000.0/lc.tempo.load(Ordering::Relaxed).max(20)as f64*e.beats)as u64;
            let start=std::time::Instant::now();
            let mut idx=0u64;
            let mut last_b_drums=u64::MAX;
            let mut last_b_bass=0u64;  // 0 = deja joue manuellement
            let mut prev_bass_note:u8=0;

            // Generation du walking bass pour cet accord
            let mut walking_notes:[u8;4]=[root,root,root,root];
            if walking {
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
                walking_notes = generate_walking_bass(&m, next_root, seed);
                seed = seed.wrapping_add(1);
            }

            // Basse : jouer le premier temps IMMEDIATEMENT (pas via le tick)
            if !t_bass.mute.load(Ordering::Relaxed) {
                let bvol=t_bass.volume.load(Ordering::Relaxed);
                let bass_note = if walking { walking_notes[0] } else { root };
                no(c,ch_bass,bass_note,bvol);
                prev_bass_note=bass_note;
                last_b_bass=0;
            }

            // Nappes (Strings)
            if !t_str.mute.load(Ordering::Relaxed) {
                for n in &prev_nappe{no(c,ch_str,*n,0)}
                let str_vol=t_str.volume.load(Ordering::Relaxed);
                for n in &nappe_notes{no(c,ch_str,*n,str_vol)}
                prev_nappe=nappe_notes.clone();
            }

            while(start.elapsed().as_millis()as u64)<dur&&!lc.stop.load(Ordering::Relaxed){
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
                        let dvol=t_drums.volume.load(Ordering::Relaxed);
                        drum_hit(c,beat,pt,true,false,bars,dvol);
                        last_b_drums=beat;
                    }
                    let beat_pos=elapsed_ms%bd_ms;
                    if beat_pos>bd_ms/2.0-10.0&&beat_pos<bd_ms/2.0+10.0{
                        let dvol=t_drums.volume.load(Ordering::Relaxed);
                        drum_hit(c,beat,pt,false,true,bars,dvol);
                    }
                }

                // Basse (Walking ou root)
                if !t_bass.mute.load(Ordering::Relaxed){
                    if beat>last_b_bass{
                        let bvol=t_bass.volume.load(Ordering::Relaxed);
                        let bass_note = if walking {
                            let bi = (beat % 4) as usize;
                            walking_notes[bi]
                        } else {
                            root
                        };
                        no(c,ch_bass,prev_bass_note,0);
                        no(c,ch_bass,bass_note,bvol);
                        prev_bass_note=bass_note;
                        last_b_bass=beat;
                    }
                }

                // Arpège (Lead)
                let tick=idx%arp.len().max(1)as u64;
                let lead_mute=t_lead.mute.load(Ordering::Relaxed);
                if !lead_mute&&!arp.is_empty(){
                    if idx>0{let p=arp[((idx-1)%arp.len()as u64)as usize];no(c,ch_lead,p,0)}
                    let lvol=t_lead.volume.load(Ordering::Relaxed);
                    no(c,ch_lead,arp[tick as usize],lvol);
                }

                idx+=1;
            }
            if lc.stop.load(Ordering::Relaxed){break}
        }
        for n in &prev_nappe{no(c,ch_str,*n,0)}
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
        if!ev.is_empty(){let sq=ev.to_vec();let l=Arc::clone(lv);
            std::thread::spawn(move||{if let Ok(mut c)=h2.lock(){play_seq(&mut c,&sq,&l,do_loop)}});
        }else if let Some(ref n)=b.notes{let v=n.clone();
            std::thread::spawn(move||{if let Ok(mut c)=h2.lock(){play_notes(&mut c,&v)}});
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
    if let Some(t)=b.tempo{lv.tempo.store(t,Ordering::Relaxed)}
    if let Some(ref sg)=b.sig{lv.sig.store(sig_code(sg),Ordering::Relaxed)}
    if let Some(iv)=b.instrument{lv.tracks[TRACK_LEAD].program.store(iv,Ordering::Relaxed)}
    if let Some(w)=b.walking{lv.walking.store(w,Ordering::Relaxed)}
    Json(Rsp{status:"ok".into()})
}

async fn stop(State(s):State<AppState>)->impl IntoResponse{
    s.live.stop.store(true,Ordering::Relaxed);
    if let Some(ref h)=s.midi{if let Ok(mut c)=h.lock(){rch(&mut c)}}
    Json(serde_json::json!({"status":"stopped"}))
}

#[tokio::main]
async fn main(){
    println!("csrs");let midi=init_midi();
    let state=AppState{midi,live:Arc::new(Live{
        tracks: [
            LiveTrack::new(0,51,15),   // Lead (canal 0)
            LiveTrack::new(2,33,40),   // Bass (canal 2)
            LiveTrack::new(3,48,30),   // Strings (canal 3)
            LiveTrack::new(9,1,80),    // Drums (canal 9)
        ],
        pattern:AtomicU8::new(PAT_ROCK),tempo:AtomicU16::new(120),stop:AtomicBool::new(false),sig:AtomicU16::new(44),
        walking:AtomicBool::new(false),
    })};
    let app=Router::new().route("/",get(idx)).route("/play",post(play))
        .route("/config",post(conf)).route("/stop",post(stop))
        .layer(CorsLayer::permissive()).with_state(state);
    let p=std::env::var("PORT").unwrap_or_else(|_|"4000".to_string());
    println!("http://0.0.0.0:{p}");
    let l=tokio::net::TcpListener::bind(format!("0.0.0.0:{p}")).await.unwrap();
    axum::serve(l,app).await.unwrap();
}
