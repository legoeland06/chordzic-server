import { BrowserSynth } from './browserSynth';
import { backendUrl, chordToNoteNames } from './chordUtils';
import { hasPlayableContent } from './playGuard';
/** Effets neutres (tout à 0). */
export const FX_ZERO = { reverb: 0, chorus: 0, delay: 0, drive: 0 };
/**
 * Constructeur de piste (factory) — défauts centralisés.
 * Symétrique de `LiveTrack::new` côté backend.
 * Toute création de piste DOIT passer par ici (un seul endroit à modifier
 * si de nouveaux paramètres sont ajoutés à TrackConfig).
 */
export function createTrack(channel, overrides = {}) {
    return {
        channel,
        label: `Piste ${channel}`,
        program: 0,
        volume: 80,
        mute: false,
        fx: { ...FX_ZERO },
        ...overrides,
    };
}
export class AudioEngine {
    constructor() {
        this.playing = false;
        this.playGen = 0; // Génération de lecture (évite les conflits)
        this.drumPattern = "rock";
        this.walking = false;
        this.tempo = 120;
        this.sig = "4/4";
        this.browserSynth = new BrowserSynth();
        this._browserAudio = false;
        this.masterVol = 127;
        /** Configuration des 5 pistes MIDI. */
        this.tracks = [
            { channel: 0, label: 'Lead', program: 51, volume: 15, mute: false },
            { channel: 2, label: 'Bass', program: 33, volume: 40, mute: false },
            { channel: 3, label: 'Nappes', program: 48, volume: 30, mute: false },
            { channel: 4, label: 'Accent', program: 2, volume: 20, mute: false },
            { channel: 9, label: 'Drums', program: 1, volume: 80, mute: false },
        ];
    }
    get browserAudio() { return this._browserAudio; }
    set browserAudio(v) { this._browserAudio = v; }
    // ── Méthodes de configuration ──
    /**
     * Modifie la configuration d'une piste et l'envoie au backend.
     * Si le canal est inconnu (piste ajoutée par l'utilisateur), la piste
     * est créée dans le moteur (pistes DYNAMIQUES).
     */
    setTrack(channel, config) {
        let t = this.tracks.find(tc => tc.channel === channel);
        if (!t) {
            // Nouvelle piste (canal inconnu) → la construire via la factory
            t = createTrack(channel, config);
            this.tracks.push(t);
        }
        Object.assign(t, config);
        this.sendConfig();
    }
    /** Ajoute une nouvelle piste (canal inconnu) au moteur et l'envoie. */
    addTrack(track) {
        if (this.tracks.some(t => t.channel === track.channel)) {
            this.setTrack(track.channel, track);
            return;
        }
        this.tracks.push({ ...track });
        this.sendConfig();
    }
    /** Supprime une piste du moteur (le backend mute le canal orphelin). */
    removeTrack(channel) {
        this.tracks = this.tracks.filter(t => t.channel !== channel);
        this.sendConfig();
    }
    setDrums(v) { this.setTrack(9, { mute: !v }); }
    setBass(v) { this.setTrack(2, { mute: !v }); }
    setArpeggios(v) { this.setTrack(0, { mute: !v }); }
    setNappes(v) { this.setTrack(3, { mute: !v }); }
    setPattern(p) { this.drumPattern = p; this.sendConfig(); }
    setSig(s) { this.sig = s; this.sendConfig(); }
    setTempo(t) { this.tempo = t; this.sendConfig({ tempo: t }); }
    setWalking(v) { this.walking = v; this.sendConfig(); }
    /** Envoie la configuration courante au backend (POST /config). */
    sendConfig(extra = {}) {
        fetch(`${backendUrl()}/config`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                tracks: this.tracks.map(t => ({
                    channel: t.channel, program: t.program, volume: t.volume, mute: t.mute,
                    drums: t.drums ?? false,
                    effects: t.fx ?? FX_ZERO,
                })),
                pattern: this.drumPattern,
                walking: this.walking,
                sig: this.sig,
                ...extra,
            }),
        }).catch(() => { });
    }
    /** Vérifie que le backend est accessible. */
    async init() {
        try {
            const resp = await fetch(backendUrl());
            if (resp.ok)
                console.log('🔌 Backend MIDI', backendUrl());
        }
        catch {
            console.warn('⚠️ Backend MIDI indisponible');
        }
    }
    setProgram(index) { this.setTrack(0, { program: index }); }
    set432Hz(enabled) { this.sendConfig({ use432: enabled }); }
    setVolume(vol) { this.masterVol = vol; this.sendConfig({ master_vol: vol }); }
    /** Enregistre un callback pour le highlight des accords. */
    onHighlight(cb) { this.onChordHighlight = cb; }
    // ── Conversion notes ──
    // La fonction `chordToNoteNames()` est importée depuis lib/chordUtils.ts.
    // ── Lecture / arrêt ──
    /**
     * Joue un aperçu d'un accord en boucle.
     * Highlight synchrone avec compensation de drift via performance.now().
     */
    async playChordPreview(chord) {
        await this.stop();
        const gen = this.playGen;
        this.playing = true;
        if (this._browserAudio) {
            await this.browserSynth.playChordPreview(chord, this.tempo, {
                tempo: this.tempo,
                pattern: this.drumPattern, walking: this.walking,
                sig: this.sig,
                master_vol: this.masterVol,
                tracks: this.tracks.map(t => ({
                    channel: t.channel, program: t.program, volume: t.volume, mute: t.mute,
                    drums: t.drums ?? false, effects: t.fx ?? FX_ZERO,
                })),
            });
        }
        else {
            const noteNames = chordToNoteNames(chord);
            const sequence = [{ notes: noteNames, beats: 4.0 }];
            try {
                const resp = await fetch(`${backendUrl()}/play`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        sequence, tempo: this.tempo, sig: this.sig,
                        pattern: this.drumPattern, walking: this.walking, loop_enabled: true,
                        tracks: this.tracks.map(t => ({
                            channel: t.channel, program: t.program, volume: t.volume, mute: t.mute,
                            drums: t.drums ?? false, effects: t.fx ?? FX_ZERO,
                        })),
                    }),
                });
                if (!resp.ok) {
                    this.playing = false;
                    return;
                }
            }
            catch {
                this.playing = false;
                return;
            }
        }
        const startTime = performance.now();
        let cumulativeExpected = 0;
        const msPerChord = (60000.0 / this.tempo) * 4;
        while (this.playing) {
            if (this.onChordHighlight)
                this.onChordHighlight(0);
            cumulativeExpected += msPerChord;
            const elapsed = performance.now() - startTime;
            const waitMs = Math.max(0, cumulativeExpected - elapsed);
            if (waitMs > 1)
                await new Promise(r => setTimeout(r, waitMs));
        }
        if (this.playGen === gen) {
            if (this.onChordHighlight)
                this.onChordHighlight(-1);
            this.playing = false;
        }
    }
    /**
     * Joue une grille complète d'accords, avec ou sans boucle.
     */
    async playGrille(grille, loop, customNotes, customChannels, loopInterval, startAtBeats, renderer) {
        await this.stop();
        const gen = this.playGen;
        this.playing = true;
        // Rien à jouer UNIQUEMENT si la grille ET les notes sont vides — un
        // projet Navig (notes seules, grille Live vide) doit pouvoir se lire.
        if (!hasPlayableContent(grille.chords.length, customNotes?.length ?? 0)) {
            this.playing = false;
            return;
        }
        if (this._browserAudio) {
            await this.browserSynth.playGrille(grille, this.tempo, loop, {
                tempo: this.tempo, pattern: this.drumPattern, walking: this.walking, sig: this.sig,
                master_vol: this.masterVol,
                tracks: this.tracks.map(t => ({
                    channel: t.channel, program: t.program, volume: t.volume, mute: t.mute,
                    drums: t.drums ?? false,
                    effects: t.fx ?? FX_ZERO,
                })),
                customNotes,
                customChannels,
                ...(loopInterval ? { loopStart: loopInterval.start, loopEnd: loopInterval.end } : {}),
                ...(startAtBeats && startAtBeats > 0 ? { startAtBeats } : {}),
                ...(renderer ? { renderer } : {}),
            });
        }
        else {
            const sequence = grille.chords.map(c => ({
                notes: chordToNoteNames(c), beats: 4.0 / c.time,
            }));
            try {
                const resp = await fetch(`${backendUrl()}/play`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        sequence, tempo: this.tempo, sig: this.sig,
                        pattern: this.drumPattern, walking: this.walking, loop_enabled: loop || false,
                        tracks: this.tracks.map(t => ({
                            channel: t.channel, program: t.program, volume: t.volume, mute: t.mute,
                            drums: t.drums ?? false,
                            effects: t.fx ?? FX_ZERO,
                        })),
                        custom_notes: customNotes || [],
                    }),
                });
                if (!resp.ok) {
                    this.playing = false;
                    return;
                }
            }
            catch {
                this.playing = false;
                return;
            }
        }
        const startTime = performance.now();
        let cumulativeExpected = 0;
        if (this._browserAudio) {
            // ── Mode Navig : highlight synchronisé sur la position audio ──
            // On lit la position réelle dans le buffer (boucle comprise) au lieu
            // d'un timer JS : plus aucune désynchronisation son/highlight.
            const dur = this.browserSynth.getDuration();
            const totalBeats = grille.chords.reduce((acc, c) => acc + 4.0 / c.time, 0);
            const beatsAcc = [];
            let acc = 0;
            for (const c of grille.chords) {
                acc += 4.0 / c.time;
                beatsAcc.push(acc);
            }
            let lastIdx = -1;
            while (this.playing) {
                const pos = dur > 0 ? this.browserSynth.getPosition() : 0;
                const beatPos = dur > 0 ? (pos / dur) * totalBeats : 0;
                let idx = 0;
                for (let i = 0; i < beatsAcc.length; i++) {
                    if (beatPos < beatsAcc[i]) {
                        idx = i;
                        break;
                    }
                    idx = i;
                }
                if (idx !== lastIdx) {
                    lastIdx = idx;
                    if (this.onChordHighlight)
                        this.onChordHighlight(idx);
                }
                await new Promise(r => setTimeout(r, 30));
            }
        }
        else {
            while (this.playing) {
                for (let idx = 0; idx < grille.chords.length && this.playing; idx++) {
                    if (this.onChordHighlight)
                        this.onChordHighlight(idx);
                    const c = grille.chords[idx];
                    const beats = 4.0 / c.time;
                    cumulativeExpected += (60000.0 / this.tempo) * beats;
                    const elapsed = performance.now() - startTime;
                    const waitMs = Math.max(0, cumulativeExpected - elapsed);
                    if (waitMs > 1)
                        await new Promise(r => setTimeout(r, waitMs));
                }
                if (!loop)
                    break;
            }
        }
        if (this.playGen === gen) {
            if (this.onChordHighlight)
                this.onChordHighlight(-1);
            this.playing = false;
        }
    }
    /** Récupère les notes du mode classique (pré-remplissage PianoRoll). */
    async getPianoNotes(grille) {
        const sequence = grille.chords.map(c => ({
            notes: chordToNoteNames(c), beats: 4.0 / c.time,
        }));
        return this.browserSynth.getPianoNotes(sequence, this.tempo, {
            tempo: this.tempo, pattern: this.drumPattern, walking: this.walking, sig: this.sig,
            tracks: this.tracks.map(t => ({
                channel: t.channel, program: t.program, volume: t.volume, mute: t.mute,
                drums: t.drums ?? false,
                effects: t.fx ?? FX_ZERO,
            })),
        });
    }
    // ── Lecture locale d'un canal PianoRoll (play/pause + curseur) ──
    /** Joue les notes d'un canal PianoRoll (rendu WAV du canal seul, mode Navig).
     * Stoppe d'abord toute lecture en cours (locale ou globale). */
    async playPianoRollChannel(channel, notes, tempo) {
        await this.stop();
        this.playing = true;
        const customNotes = notes.map(n => ({
            channel,
            start_time: n.startTime,
            pitch: n.pitch,
            duration: n.duration,
            velocity: n.velocity,
        }));
        // Tous les canaux en mode custom (les autres vides) → seul `channel` est rendu
        const customChannels = [0, 2, 3, 4, 9];
        await this.browserSynth.playPianoRollChannel(customNotes, customChannels, tempo, {
            tempo: this.tempo,
            pattern: this.drumPattern, walking: this.walking, sig: this.sig,
            master_vol: this.masterVol,
            tracks: this.tracks.map(t => ({
                channel: t.channel, program: t.program, volume: t.volume, mute: t.mute,
                drums: t.drums ?? false,
                effects: t.fx ?? FX_ZERO,
            })),
        });
    }
    /** Pause / reprise de la lecture locale (gèle le contexte audio). */
    async pausePianoRoll() { await this.browserSynth.pause(); }
    async resumePianoRoll() { await this.browserSynth.resume(); }
    /** Vrai tant que la lecture locale produit du son (faux quand le buffer est fini). */
    get pianoRollActive() { return this.browserSynth.isPlaying; }
    /** Position (secondes) et durée (secondes) de la lecture locale. */
    getPianoRollPosition() { return this.browserSynth.getPosition(); }
    getPianoRollDuration() { return this.browserSynth.getDuration(); }
    /** Position brute (sans modulo) : -1 si aucune source active. */
    getPianoRollPositionRaw() { return this.browserSynth.getPositionRaw(); }
    /** Joue le buffer Navig courant depuis une position (scrub + lecture). */
    playNavigFrom(seconds, loop) {
        this.browserSynth.playBufferFrom(seconds, loop);
    }
    /** Intervalle de boucle (locators) en secondes — appliqué aux sources
     * Web Audio locales (loopStart/loopEnd) et au wrap de position. */
    setLoopInterval(startSec, endSec) {
        this.browserSynth.setLoopInterval(startSec, endSec);
    }
    /** Déplace la tête de lecture du rendu Navig courant (lecture ou pause). */
    seekNavig(seconds) {
        this.browserSynth.seekTo(seconds);
    }
    /** Configure la boucle sample du mode Navig (jouée par le navigateur en
     * parallèle du WAV principal). Appliquée en direct pendant la lecture. */
    setSampleLoop(cfg) {
        this.browserSynth.setSampleLoop(cfg);
    }
    /** Dernier WAV rendu par le backend (mode Navig), pour extraction. */
    getLastWavBlob() { return this.browserSynth.getLastWavBlob(); }
    /** WAV à extraire — avec le sample bouclé MIXÉ si la boucle est active. */
    getExtractWavBlob() { return this.browserSynth.getExtractWavBlob(); }
    /** Joue une note en direct (preview PianoRoll) via le backend.
     * Fire-and-forget : ne bloque jamais l'édition. */
    async playPreviewNote(channel, pitch) {
        try {
            await fetch(`${backendUrl()}/note`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ channel, pitch }),
            });
        }
        catch { /* silencieux */ }
    }
    async stop() {
        this.playing = false;
        this.playGen++;
        if (this.onChordHighlight)
            this.onChordHighlight(-1);
        this.browserSynth.stop();
        try {
            await fetch(`${backendUrl()}/stop`, { method: 'POST' });
        }
        catch { }
    }
    get isPlaying() { return this.playing; }
}
/** Liste complète des instruments GM (128 noms). */
AudioEngine.INSTRUMENTS = [
    'Acoustic Grand Piano', 'Bright Acoustic Piano', 'Electric Grand Piano', 'Honky-tonk Piano',
    'Electric Piano 1', 'Electric Piano 2', 'Harpsichord', 'Clavinet',
    'Celesta', 'Glockenspiel', 'Music Box', 'Vibraphone',
    'Marimba', 'Xylophone', 'Tubular Bells', 'Dulcimer',
    'Drawbar Organ', 'Percussive Organ', 'Rock Organ', 'Church Organ',
    'Reed Organ', 'Accordion', 'Harmonica', 'Tango Accordion',
    'Acoustic Guitar (nylon)', 'Acoustic Guitar (steel)', 'Electric Guitar (jazz)', 'Electric Guitar (clean)',
    'Electric Guitar (muted)', 'Overdriven Guitar', 'Distortion Guitar', 'Guitar Harmonics',
    'Acoustic Bass', 'Electric Bass (finger)', 'Electric Bass (pick)', 'Fretless Bass',
    'Slap Bass 1', 'Slap Bass 2', 'Synth Bass 1', 'Synth Bass 2',
    'Violin', 'Viola', 'Cello', 'Contrabass',
    'Tremolo Strings', 'Pizzicato Strings', 'Orchestral Harp', 'Timpani',
    'String Ensemble 1', 'String Ensemble 2', 'Synth Strings 1', 'Synth Strings 2',
    'Choir Aahs', 'Voice Oohs', 'Synth Voice', 'Orchestra Hit',
    'Trumpet', 'Trombone', 'Tuba', 'Muted Trumpet',
    'French Horn', 'Brass Section', 'Synth Brass 1', 'Synth Brass 2',
    'Soprano Sax', 'Alto Sax', 'Tenor Sax', 'Baritone Sax',
    'Oboe', 'English Horn', 'Bassoon', 'Clarinet',
    'Piccolo', 'Flute', 'Recorder', 'Pan Flute',
    'Blown Bottle', 'Shakuhachi', 'Whistle', 'Ocarina',
    'Lead 1 (square)', 'Lead 2 (sawtooth)', 'Lead 3 (calliope)', 'Lead 4 (chiff)',
    'Lead 5 (charang)', 'Lead 6 (voice)', 'Lead 7 (fifths)', 'Lead 8 (bass + lead)',
    'Pad 1 (new age)', 'Pad 2 (warm)', 'Pad 3 (polysynth)', 'Pad 4 (choir)',
    'Pad 5 (bowed)', 'Pad 6 (metallic)', 'Pad 7 (halo)', 'Pad 8 (sweep)',
    'FX 1 (rain)', 'FX 2 (soundtrack)', 'FX 3 (crystal)', 'FX 4 (atmosphere)',
    'FX 5 (brightness)', 'FX 6 (goblins)', 'FX 7 (echoes)', 'FX 8 (sci-fi)',
    'Sitar', 'Banjo', 'Shamisen', 'Koto',
    'Kalimba', 'Bag pipe', 'Fiddle', 'Shanai',
    'Tinkle Bell', 'Agogo', 'Steel Drums', 'Woodblock',
    'Taiko Drum', 'Melodic Tom', 'Synth Drum', 'Reverse Cymbal',
    'Guitar Fret Noise', 'Breath Noise', 'Seashore', 'Bird Tweet',
    'Telephone Ring', 'Helicopter', 'Applause', 'Gunshot',
];
