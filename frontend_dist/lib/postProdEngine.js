import { backendUrl } from './chordUtils';
export class PostProdEngine {
    constructor() {
        this.session = null;
        /** Callback de niveaux (VU) : (levels par piste, master). Appelé par la vue. */
        this.onLevels = null;
        this.ctx = null;
        this.masterGain = null;
        this.sources = [];
        this.analysers = [];
        this._playing = false;
        this._pos = 0;
        this.posStart = 0;
        this.ctxStartTime = 0;
        this._loop = false;
        this.masterVolume = 1;
    }
    get isPlaying() { return this._playing; }
    get loop() { return this._loop; }
    /** Volume master (multiplicateur appliqué par-dessus session.masterGain). */
    setMasterVolume(v) {
        this.masterVolume = v;
        if (this.masterGain)
            this.masterGain.gain.value = v * (this.session?.masterGain ?? 1);
    }
    loadSession(session) {
        this.stop();
        this.session = session;
        this._pos = 0;
    }
    /** Met à jour la session sans stopper la lecture (mutations d'édition). */
    setSession(session) {
        this.session = session;
    }
    async getCtx() {
        if (!this.ctx)
            this.ctx = new AudioContext();
        if (this.ctx.state === 'suspended') {
            try {
                await this.ctx.resume();
            }
            catch { /* silencieux */ }
        }
        return this.ctx;
    }
    /** Télécharge et décode un WAV (bounce multitrack) en AudioBuffer.
     * Les URLs relatives (ex: /rendered/…) sont résolues contre le backend. */
    async decodeWav(url) {
        const full = url.startsWith('http') ? url : `${backendUrl()}${url}`;
        const resp = await fetch(full);
        if (!resp.ok)
            throw new Error(`Téléchargement impossible : ${full} (${resp.status})`);
        const data = await resp.arrayBuffer();
        return this.decodeArrayBuffer(data);
    }
    /** Décode des données audio brutes (fichier importé : WAV, MP3, FLAC…). */
    async decodeArrayBuffer(data) {
        const ctx = await this.getCtx();
        return ctx.decodeAudioData(data);
    }
    /** Tracks réellement audibles : solo prioritaire sur mute. */
    audibleTracks() {
        if (!this.session)
            return [];
        const hasSolo = this.session.tracks.some(t => t.solo);
        return this.session.tracks.filter(t => !t.mute && (!hasSolo || t.solo));
    }
    /**
     * Construit le graphe de lecture dans `ctx` (temps réel ou offline).
     * Un source par clip, `start(when, offset, duration)` : les régions sont
     * jouées exactement, sans copie de données. Les fades sont des rampes sur
     * le gain du clip. `startSec` = position de départ (découpe les clips en
     * cours de lecture).
     */
    buildGraph(ctx, when, startSec, isOffline) {
        const session = this.session;
        if (!session)
            return;
        this.sources = [];
        this.analysers = [];
        // Master : gain par défaut (fidélité au mix Navig) × volume master
        const mg = ctx.createGain();
        mg.gain.value = this.masterVolume * session.masterGain;
        mg.connect(ctx.destination);
        this.masterGain = mg;
        for (const t of this.audibleTracks()) {
            const trackGain = ctx.createGain();
            trackGain.gain.value = t.volume;
            const panner = ctx.createStereoPanner();
            panner.pan.value = Math.max(-1, Math.min(1, t.pan));
            trackGain.connect(panner);
            panner.connect(mg);
            // VU : analyser en parallèle (temps réel uniquement)
            if (!isOffline) {
                const an = ctx.createAnalyser();
                an.fftSize = 512;
                an.smoothingTimeConstant = 0.4;
                trackGain.connect(an);
                this.analysers.push(an);
            }
            for (const clip of t.clips) {
                if (clip.duration <= 0.001)
                    continue;
                const clipEnd = clip.start + clip.duration;
                if (clipEnd <= startSec)
                    continue; // clip entièrement avant le départ
                const sessionDur = this.getDuration();
                if (clip.start >= sessionDur)
                    continue;
                const cutIn = Math.max(0, startSec - clip.start);
                const offset = clip.offset + cutIn;
                const remaining = clip.duration - cutIn;
                // Tronquer à la fin de la session
                const maxDur = Math.max(0, sessionDur - (clip.start + cutIn));
                const finalDur = Math.min(remaining, maxDur || remaining);
                if (finalDur <= 0.001)
                    continue;
                const src = ctx.createBufferSource();
                src.buffer = t.buffer;
                src.loop = false;
                src.start(when, offset, finalDur);
                this.sources.push(src);
                // Gain du clip + fades (rampes en temps absolu du contexte)
                const cg = ctx.createGain();
                const g = clip.gain;
                const t0 = when;
                const tEnd = t0 + finalDur;
                const fi = Math.min(clip.fadeIn, finalDur / 2);
                const fo = Math.min(clip.fadeOut, finalDur / 2);
                if (cutIn > 0.001) {
                    // Départ en plein clip : pas de rampe d'entrée
                    cg.gain.setValueAtTime(g, t0);
                }
                else if (fi > 0.001) {
                    cg.gain.setValueAtTime(0, t0);
                    cg.gain.linearRampToValueAtTime(g, t0 + fi);
                }
                else {
                    cg.gain.setValueAtTime(g, t0);
                }
                if (fo > 0.001) {
                    cg.gain.setValueAtTime(g, Math.max(t0 + fi, tEnd - fo));
                    cg.gain.linearRampToValueAtTime(0, tEnd);
                }
                src.connect(cg);
                cg.connect(trackGain);
                src.onended = () => {
                    const idx = this.sources.indexOf(src);
                    if (idx >= 0)
                        this.sources.splice(idx, 1);
                };
            }
        }
    }
    /** Lance la lecture depuis la position courante. */
    async play(loop = false) {
        const ctx = await this.getCtx();
        this.stopSources();
        this._loop = loop;
        const when = ctx.currentTime + 0.05;
        this.buildGraph(ctx, when, this._pos, false);
        this.posStart = this._pos;
        this.ctxStartTime = when;
        this._playing = true;
    }
    pause() {
        if (this.ctx && this.ctx.state === 'running') {
            this.ctx.suspend().catch(() => { });
        }
    }
    resume() {
        if (this.ctx && this.ctx.state === 'suspended') {
            this.ctx.resume().catch(() => { });
        }
    }
    stop() {
        this.stopSources();
        this._playing = false;
        this._pos = 0;
    }
    /** Déplace la tête de lecture (arrêt ou lecture → le graphe est rebâti). */
    async seek(sec) {
        const s = Math.max(0, Math.min(sec, this.getDuration()));
        this._pos = s;
        if (this._playing) {
            const ctx = await this.getCtx();
            this.stopSources();
            const when = ctx.currentTime + 0.05;
            this.buildGraph(ctx, when, s, false);
            this.posStart = s;
            this.ctxStartTime = when;
        }
    }
    stopSources() {
        for (const s of this.sources) {
            try {
                s.stop();
            }
            catch { /* déjà arrêté */ }
            s.disconnect();
        }
        this.sources = [];
        for (const a of this.analysers)
            a.disconnect();
        this.analysers = [];
        if (this.masterGain) {
            this.masterGain.disconnect();
            this.masterGain = null;
        }
    }
    /** Position de lecture (secondes), boucle comprise. */
    getPosition() {
        const dur = this.getDuration();
        if (this._playing && this.ctx) {
            let p = this.posStart + (this.ctx.currentTime - this.ctxStartTime);
            if (this._loop && dur > 0)
                p = ((p % dur) + dur) % dur;
            return Math.max(0, Math.min(p, dur));
        }
        return Math.max(0, Math.min(this._pos, dur));
    }
    /** Durée totale effective : la session, OU la fin du dernier clip si un
     * clip a été déplacé/étiré au-delà (la timeline s'étend, comme les DAW). */
    getDuration() {
        const s = this.session;
        if (!s)
            return 0;
        let dur = s.durationSec;
        for (const t of s.tracks) {
            for (const c of t.clips)
                dur = Math.max(dur, c.start + c.duration);
        }
        return dur;
    }
    /** Niveaux VU : 0..1 par piste audible + master (max). */
    getLevels() {
        const levels = this.analysers.map(an => {
            const buf = new Uint8Array(an.fftSize);
            an.getByteTimeDomainData(buf);
            let m = 0;
            for (let i = 0; i < buf.length; i++) {
                const v = Math.abs(buf[i] - 128) / 128;
                if (v > m)
                    m = v;
            }
            return m;
        });
        const master = levels.length > 0 ? Math.max(...levels) : 0;
        return { levels, master };
    }
    /** Export du mix complet en WAV stéréo 16-bit (rendu hors-ligne). */
    async exportWav() {
        const session = this.session;
        if (!session)
            throw new Error('Aucune session à exporter');
        const wasPlaying = this._playing;
        const wasPos = this._pos;
        if (wasPlaying)
            this.pause();
        const sr = 44100;
        const len = Math.max(1, Math.ceil(this.getDuration() * sr));
        const ctx = new OfflineAudioContext(2, len, sr);
        this._pos = 0;
        this.buildGraph(ctx, 0, 0, true);
        const rendered = await ctx.startRendering();
        // Restaurer l'état de lecture
        this._pos = wasPos;
        if (wasPlaying)
            this.resume();
        return encodeWav(rendered, sr);
    }
}
/** Encode un AudioBuffer en WAV PCM 16-bit (stéréo/mono). */
function encodeWav(buffer, sampleRate) {
    const numCh = Math.min(2, buffer.numberOfChannels);
    const numFrames = buffer.length;
    const blockAlign = numCh * 2;
    const dataSize = numFrames * blockAlign;
    const ab = new ArrayBuffer(44 + dataSize);
    const view = new DataView(ab);
    const writeStr = (off, s) => {
        for (let i = 0; i < s.length; i++)
            view.setUint8(off + i, s.charCodeAt(i));
    };
    writeStr(0, 'RIFF');
    view.setUint32(4, 36 + dataSize, true);
    writeStr(8, 'WAVE');
    writeStr(12, 'fmt ');
    view.setUint32(16, 16, true);
    view.setUint16(20, 1, true); // PCM
    view.setUint16(22, numCh, true);
    view.setUint32(24, sampleRate, true);
    view.setUint32(28, sampleRate * blockAlign, true);
    view.setUint16(32, blockAlign, true);
    view.setUint16(34, 16, true);
    writeStr(36, 'data');
    view.setUint32(40, dataSize, true);
    const chans = [];
    for (let c = 0; c < numCh; c++)
        chans.push(buffer.getChannelData(c));
    let off = 44;
    for (let i = 0; i < numFrames; i++) {
        for (let c = 0; c < numCh; c++) {
            const s = Math.max(-1, Math.min(1, chans[c][i]));
            view.setInt16(off, s < 0 ? s * 0x8000 : s * 0x7FFF, true);
            off += 2;
        }
    }
    return new Blob([ab], { type: 'audio/wav' });
}
