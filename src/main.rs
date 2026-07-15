use axum::{extract::State, response::{Html, Json, IntoResponse}, routing::{get, post}, Router};
use midir::{MidiOutput, MidiOutputConnection};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower_http::cors::CorsLayer;

type MidiHandle = Arc<Mutex<MidiOutputConnection>>;
const PAT_ROCK:u8=0; const PAT_REGGAE:u8=1; const PAT_JAZZ:u8=2;
fn pat(s:&str)->u8{match s{"reggae"=>PAT_REGGAE,"jazz"=>PAT_JAZZ,_=>PAT_ROCK}}

struct Live{
    drums:AtomicBool, bass:AtomicBool, arpeggios:AtomicBool,
    pattern:AtomicU8, tempo:AtomicU16, stop:AtomicBool,
}
#[derive(Clone)] struct AppState{midi:Option<MidiHandle>,live:Arc<Live>}

#[derive(Deserialize)]
struct PlayReq{
    notes:Option<Vec<String>>,#[serde(default)]seq:Vec<ChordEv>,#[serde(default)]sequence:Vec<ChordEv>,
    #[serde(default="t120")]tempo:u32,#[serde(default="y")]drums:bool,
    #[serde(default="y")]bass:bool,#[serde(default="y")]arps:bool,#[serde(default="rk")]pattern:String,
}
#[derive(Deserialize,Clone)] struct ChordEv{notes:Vec<String>,#[serde(default="b4")]beats:f64}
#[derive(Serialize)] struct Rsp{status:String}
#[derive(Deserialize)] struct Cfg{
    drums:Option<bool>,bass:Option<bool>,arpeggios:Option<bool>,
    pattern:Option<String>,tempo:Option<u16>,
}
fn t120()->u32{120}fn y()->bool{true}fn rk()->String{"rock".to_string()}fn b4()->f64{4.0}

fn note_midi(s:&str)->Result<u8,String>{
    let s=s.trim();let(nl,np)=if s.len()>1&&(s.as_bytes()[1]==b'#'||s.as_bytes()[1]==b'b'){(2,&s[..2])}else{(1,&s[..1])};
    let o:i32=s[nl..].parse().map_err(|_|"o")?;let u=np.to_uppercase();
    let n=match u.as_str(){"DB"=>"C#","EB"=>"D#","GB"=>"F#","AB"=>"G#","BB"=>"A#",_=>&u};
    let i=["C","C#","D","D#","E","F","F#","G","G#","A","A#","B"].iter().position(|x|x==&n).ok_or("?")?;
    let m=(o+1)*12+i as i32;if m<0||m>127{return Err("o".into())}Ok(m as u8)
}

fn init_midi()->Option<MidiHandle>{
    let mo=MidiOutput::new("cs").ok()?;let p=mo.ports();
    if p.is_empty(){eprintln!("no port");return None}
    println!("Ports:");for(i,x)in p.iter().enumerate(){if let Ok(n)=mo.port_name(x){println!(" [{i}] {n}")}}
    let i:usize=if let Ok(e)=std::env::var("MIDI_PORT"){e.parse().unwrap_or(0)}
    else if let Some((i,_))=p.iter().enumerate().find(|(_,x)|mo.port_name(x).map(|n|n.contains("Roland")).unwrap_or(false)){println!("  Roland");i}
    else{p.len().saturating_sub(1)};
    if i>=p.len(){eprintln!("port {i} invalid");return None}
    println!("Connecte {}",mo.port_name(&p[i]).unwrap_or_default());
    mo.connect(&p[i],"cs").ok().map(|c|Arc::new(Mutex::new(c)))
}

fn snd(c:&mut MidiOutputConnection,m:&[u8]){if let Err(e)=c.send(m){eprintln!("⚠️{e}")}}
fn cc(c:&mut MidiOutputConnection,ch:u8,ctl:u8,v:u8){snd(c,&[0xB0|ch,ctl,v])}
fn pc(c:&mut MidiOutputConnection,ch:u8,v:u8){snd(c,&[0xC0|ch,v])}
fn no(c:&mut MidiOutputConnection,ch:u8,n:u8,v:u8){snd(c,&[0x90|ch,n,v])}
fn rch(c:&mut MidiOutputConnection){for&ch in&[0u8,1,2,9]{cc(c,ch,123,0)}}

const DRUM_KICK:u8=36; const DRUM_SNARE:u8=38; const DRUM_RIM:u8=37;
const DRUM_HH:u8=42; const DRUM_RIDE:u8=51;
const HH_BEAT:u8=80; // vélocité HH sur les temps
const HH_8TH:u8=65;  // vélocité HH sur les croches

fn drum_hit(c:&mut MidiOutputConnection,beat:u64,pat:u8,on_beat:bool,on_eighth:bool){
    if!on_beat&&!on_eighth{return}
    let b=beat%4;
    match pat{
        PAT_REGGAE=>if on_beat{match b{
            0=>{no(c,9,DRUM_HH,60);}1=>{no(c,9,DRUM_RIM,70);no(c,9,DRUM_HH,60);}
            2=>{no(c,9,DRUM_KICK,85);no(c,9,DRUM_HH,65);}3=>{no(c,9,DRUM_RIM,70);no(c,9,DRUM_HH,60);}_=>{}
        }}else if on_eighth{no(c,9,DRUM_HH,55);}
        PAT_JAZZ=>if on_beat{match b{
            0=>{no(c,9,DRUM_RIDE,60);no(c,9,DRUM_KICK,50);}1=>{no(c,9,DRUM_RIDE,60);no(c,9,DRUM_SNARE,55);}
            2=>{no(c,9,DRUM_RIDE,60);no(c,9,DRUM_KICK,50);}3=>{no(c,9,DRUM_RIDE,60);no(c,9,DRUM_SNARE,55);}_=>{}
        }}else if on_eighth{no(c,9,DRUM_HH,35);}
        _=>if on_beat{match b{
            0=>{no(c,9,DRUM_KICK,90);no(c,9,DRUM_HH,HH_BEAT);}1=>{no(c,9,DRUM_SNARE,75);no(c,9,DRUM_HH,HH_BEAT);}
            2=>{no(c,9,DRUM_KICK,80);no(c,9,DRUM_HH,HH_BEAT);}3=>{no(c,9,DRUM_SNARE,70);no(c,9,DRUM_HH,HH_BEAT);}_=>{}
        }}else if on_eighth{no(c,9,DRUM_HH,HH_8TH);}
    }
}

fn play_notes(c:&mut MidiOutputConnection,notes:&[String]){
    let mut v:Vec<u8>=vec![];for n in notes{if let Ok(m)=note_midi(n){v.push(m)}}if v.is_empty(){return}
    rch(c);
    for&ch in&[0u8,1,2]{cc(c,ch,101,0);cc(c,ch,100,1);cc(c,ch,6,62);cc(c,ch,38,2)}
    pc(c,0,51);pc(c,1,24);pc(c,2,50);
    for _ in 0..2{for&n in&v{std::thread::sleep(Duration::from_millis(240));if n<48{no(c,2,n,35)}else{no(c,0,n,15);no(c,1,n,15)}}}
    rch(c);println!("  notes: {v:?}");
}

fn play_seq(c:&mut MidiOutputConnection,ev:&[ChordEv],cfg:&Live){
    rch(c);
    for&ch in&[0u8,1,2]{cc(c,ch,101,0);cc(c,ch,100,1);cc(c,ch,6,62);cc(c,ch,38,2)}
    pc(c,0,51);pc(c,1,24);pc(c,2,50);
    pc(c,9,1); // Standard Kit
    std::thread::sleep(Duration::from_millis(2));

    for(i,e)in ev.iter().enumerate(){
        let mut m:Vec<u8>=vec![];for n in&e.notes{if let Ok(x)=note_midi(n){m.push(x)}}
        if m.is_empty(){continue}
        if i>0{rch(c)}

        let root=m[0];let arp:Vec<u8>=m[1..].to_vec();
        let dur=(60_000.0/cfg.tempo.load(Ordering::Relaxed).max(20)as f64*e.beats)as u64;
        let start=std::time::Instant::now();
        let mut idx=0u64;
        let mut last_b=u64::MAX;

        while(start.elapsed().as_millis()as u64)<dur&&!cfg.stop.load(Ordering::Relaxed){
            let tempo_f=cfg.tempo.load(Ordering::Relaxed).max(20)as f64;
            let bd_ms=60_000.0/tempo_f;
            let delay_ms=(bd_ms/4.0).max(30.0);

            // Timer compensé: target absolu basé sur idx (f64)
            let target=start+Duration::from_secs_f64(idx as f64*delay_ms/1000.0);
            let now=std::time::Instant::now();
            if target>now{std::thread::sleep(target-now)}

            let elapsed_ms=start.elapsed().as_secs_f64()*1000.0;
            let dr=cfg.drums.load(Ordering::Relaxed);
            let ba=cfg.bass.load(Ordering::Relaxed);
            let ar=cfg.arpeggios.load(Ordering::Relaxed);
            let pt=cfg.pattern.load(Ordering::Relaxed);

            // Batterie: beat basé sur elapsed f64
            if dr{
                let beat=(elapsed_ms/bd_ms)as u64;
                if last_b==u64::MAX||beat>last_b{
                    drum_hit(c,beat,pt,true,false);
                    last_b=beat;
                }
                // Croche: vérifiée à chaque tick, indépendamment du beat
                let beat_pos=elapsed_ms%bd_ms;
                if beat_pos>bd_ms/2.0-10.0&&beat_pos<bd_ms/2.0+10.0{
                    drum_hit(c,beat,pt,false,true);
                }
            }

            // Arpège
            let tick=idx%arp.len().max(1)as u64;
            if ar&&!arp.is_empty(){
                if idx>0{let p=arp[((idx-1)%arp.len()as u64)as usize];no(c,0,p,0);no(c,1,p,0)}
                no(c,0,arp[tick as usize],15);no(c,1,arp[tick as usize],15);
            }

            // Basse
            if ba&&idx%4==0{
                if idx>=4{no(c,2,root,0)}
                no(c,2,root,40);
            }

            idx+=1;
        }
        if cfg.stop.load(Ordering::Relaxed){break}
    }
    rch(c);
    println!("  done ({} evts)",ev.len());
}

async fn idx()->impl IntoResponse{Html(include_str!("../static/index.html"))}

async fn play(State(s):State<AppState>,Json(b):Json<PlayReq>)->impl IntoResponse{
    s.live.drums.store(b.drums,Ordering::Relaxed);
    s.live.bass.store(b.bass,Ordering::Relaxed);
    s.live.arpeggios.store(b.arps,Ordering::Relaxed);
    s.live.pattern.store(pat(&b.pattern),Ordering::Relaxed);
    s.live.tempo.store(b.tempo as u16,Ordering::Relaxed);
    s.live.stop.store(false,Ordering::Relaxed);
    let ev:&[ChordEv]=if!b.seq.is_empty(){b.seq.as_slice()}else if!b.sequence.is_empty(){b.sequence.as_slice()}else{&[]};
    if let Some(ref h)=s.midi{
        let h2=Arc::clone(h);
        if!ev.is_empty(){let sq=ev.to_vec();let l=Arc::clone(&s.live);
            std::thread::spawn(move||{if let Ok(mut c)=h2.lock(){play_seq(&mut c,&sq,&l)}});
        }else if let Some(ref n)=b.notes{let v=n.clone();
            std::thread::spawn(move||{if let Ok(mut c)=h2.lock(){play_notes(&mut c,&v)}});
        }
    }
    Json(Rsp{status:"ok".into()})
}

async fn conf(State(s):State<AppState>,Json(b):Json<Cfg>)->impl IntoResponse{
    if let Some(v)=b.drums{s.live.drums.store(v,Ordering::Relaxed)}
    if let Some(v)=b.bass{s.live.bass.store(v,Ordering::Relaxed)}
    if let Some(v)=b.arpeggios{s.live.arpeggios.store(v,Ordering::Relaxed)}
    if let Some(ref p)=b.pattern{s.live.pattern.store(pat(p),Ordering::Relaxed)}
    if let Some(t)=b.tempo{s.live.tempo.store(t,Ordering::Relaxed)}
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
        drums:AtomicBool::new(true),bass:AtomicBool::new(true),arpeggios:AtomicBool::new(true),
        pattern:AtomicU8::new(PAT_ROCK),tempo:AtomicU16::new(120),stop:AtomicBool::new(false),
    })};
    let app=Router::new().route("/",get(idx)).route("/play",post(play))
        .route("/config",post(conf)).route("/stop",post(stop))
        .layer(CorsLayer::permissive()).with_state(state);
    let p=std::env::var("PORT").unwrap_or_else(|_|"4000".to_string());
    println!("http://0.0.0.0:{p}");
    let l=tokio::net::TcpListener::bind(format!("0.0.0.0:{p}")).await.unwrap();
    axum::serve(l,app).await.unwrap();
}
