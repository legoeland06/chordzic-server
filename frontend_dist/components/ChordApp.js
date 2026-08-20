import { jsx as _jsx, jsxs as _jsxs, Fragment as _Fragment } from "react/jsx-runtime";
/**
 * ChordApp — composant principal de l'application chordZIC.
 *
 * Orchestre la grille d'accords, le moteur audio, la lecture synchrone,
 * la sauvegarde/chargement, l'export/import JSON et les modals.
 *
 * État géré entièrement via useState/useRef, l'AudioEngine est une instance
 * unique dans un useRef.
 */
import { useState, useRef, useCallback, useEffect, useMemo } from 'react';
import { Sparkles, Music } from 'lucide-react';
import { parseChord, parseGrille } from '../types/chord';
import { hasPlayableContent, hasSaveableContent } from '../lib/playGuard';
import { AudioEngine, createTrack, FX_ZERO } from '../lib/audioEngine';
import { DEFAULT_SAMPLE_VOLUME } from '../lib/sampleLoop';
import ChordInput from './ChordInput';
import ControlBar from './ControlBar';
import LiveSettingsBar from './LiveSettingsBar';
import ProgressBar from './ProgressBar';
import ChordGrid from './ChordGrid';
import PianoLivePanel from './PianoLivePanel';
import { sendPianoNote } from '../lib/pianoNote';
import ChordNowModal from './ChordNowModal';
import ChordDetailModal from './ChordDetailModal';
import { SaveModal, LoadModal, NewProjectModal } from './SaveLoadModal';
import HelpModal from './HelpModal';
import DawView from './DawView';
import PostProdView from './PostProdView';
import { PostProdEngine } from '../lib/postProdEngine';
import { createFullClip, trackColorForChannel } from '../lib/postProdTypes';
import { backendUrl, chordToNoteNames } from '../lib/chordUtils';
const API_BASE = backendUrl();
// Clé localStorage des ANCIENNES grilles (migration unique vers le serveur)
const STORAGE_KEY = 'chordjava_saved_grilles';
/** Clé localStorage de l'AUTO-SAUVEGARDE locale (anti-perte à l'actualisation). */
const AUTOSAVE_KEY = 'chordzic_autosave_v1';
/** Délai (ms) avant écriture de l'auto-sauvegarde après un changement. */
const AUTOSAVE_DEBOUNCE_MS = 800;
export default function ChordApp() {
    // ── État : grille d'accords ──────────────────────────────────────
    const [input, setInput] = useState('1:Fm9 1:Cm9 1:Gm9 1:Dm7');
    const [chords, setChords] = useState([]);
    const [highlighted, setHighlighted] = useState(-1);
    const [playing, setPlaying] = useState(false);
    const [currentBeat, setCurrentBeat] = useState(0);
    // ── État : paramètres audio ──────────────────────────────────────
    const [tempo, setTempo] = useState(120);
    const [volume, setVolume] = useState(127);
    const [use432, setUse432] = useState(false);
    const [browserAudio, setBrowserAudio] = useState(false);
    /** Vrai dès qu'un WAV a été rendu en mode Navig (bouton Extract actif). */
    const [hasWav, setHasWav] = useState(false);
    /** Session audio du mode PostProd (bounce multitrack) — null tant qu'aucun bounce. */
    const [postProdSession, setPostProdSession] = useState(null);
    /** Vrai quand la vue PostProd est affichée (bascule depuis Navig). */
    const [showPostProd, setShowPostProd] = useState(false);
    /** Vrai pendant le bounce multitrack (bouton PostProd désactivé). */
    const [bouncing, setBouncing] = useState(false);
    /** Pistes par défaut d'un nouveau projet — référence stable (le reset
     * « Nouveau projet » repart toujours de cette liste). */
    const DEFAULT_TRACKS = [
        { channel: 0, label: 'Lead', program: 51, volume: 60, mute: false },
        { channel: 2, label: 'Bass', program: 33, volume: 70, mute: false },
        { channel: 3, label: 'Nappes', program: 48, volume: 60, mute: false },
        { channel: 9, label: 'Drums', program: 1, volume: 90, mute: false },
        { channel: 4, label: 'Accent', program: 2, volume: 50, mute: false },
    ];
    const [tracks, setLocalTracks] = useState(DEFAULT_TRACKS);
    const updateTrack = (channel, cfg) => {
        setLocalTracks(prev => {
            if (prev.some(t => t.channel === channel)) {
                return prev.map(t => t.channel === channel ? { ...t, ...cfg } : t);
            }
            // Piste inconnue (ex: grille chargée avec des pistes ajoutées) → l'ajouter
            return [...prev, createTrack(channel, cfg)];
        });
        engineRef.current?.setTrack(channel, cfg);
    };
    /** REMPLACE la liste des pistes par celle d'une grille chargée (Load /
     * Import / restauration autosave) : les pistes du projet courant absentes
     * de la grille sont retirées (plus de pistes orphelines vides), et le
     * moteur AudioEngine est synchronisé (canaux retirés + pistes appliquées). */
    const applyLoadedTracks = (loaded) => {
        const nextTracks = loaded.map(tc => createTrack(tc.channel, tc));
        const nextChannels = new Set(nextTracks.map(t => t.channel));
        // Retirer du moteur les canaux qui ne sont plus dans la grille
        for (const t of tracks) {
            if (!nextChannels.has(t.channel))
                engineRef.current?.removeTrack(t.channel);
        }
        setLocalTracks(nextTracks);
        for (const t of nextTracks)
            engineRef.current?.setTrack(t.channel, t);
    };
    /** Vrai pendant un chargement de grille : l'auto-config Reggae est suspendue
     * (elle écraserait les instruments sauvegardés — bug « Load 2 fois »). */
    const suppressAutoConfigRef = useRef(false);
    // ── Pistes dynamiques : ajout / suppression ────────────────────────
    /** Canaux MIDI proposés pour une nouvelle piste instrument (le 9 est réservé aux drums). */
    const AVAILABLE_CHANNELS = [1, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15];
    /** Choix affiché à l'ajout de piste (null = modale fermée). */
    const [showAddTrack, setShowAddTrack] = useState(false);
    /** Ajoute une nouvelle piste — l'utilisateur choisit le type :
     * 'inst'  = instrument mélodique (canaux 1, 5-8, 10-15)
     * 'drums' = percussion / kit drums (canal 9 s'il est libre, sinon canaux
     *           libres — le backend programme la banque percussion GM2). */
    const addTrack = (kind) => {
        const used = new Set(tracks.map(t => t.channel));
        let ch;
        if (kind === 'drums') {
            // Canal drums GM (9) prioritaire s'il est libre, sinon canal libre
            ch = !used.has(9) ? 9 : AVAILABLE_CHANNELS.find(c => !used.has(c));
        }
        else {
            ch = AVAILABLE_CHANNELS.find(c => !used.has(c));
        }
        if (ch === undefined) {
            setStatus('❌ Tous les canaux MIDI sont utilisés');
            setStatusColor('text-red-400');
            setShowAddTrack(false);
            return;
        }
        const isDrum = kind === 'drums';
        const newTrack = createTrack(ch, {
            drums: isDrum,
            label: isDrum ? `Drums ${ch}` : `Piste ${ch}`,
        });
        setLocalTracks(prev => [...prev, newTrack]);
        engineRef.current?.addTrack(newTrack);
        setShowAddTrack(false);
        setStatus(`➕ Piste « ${newTrack.label} » ajoutée (canal ${ch}, ${isDrum ? 'drums' : 'instrument'})`);
        setStatusColor('text-blue-400');
    };
    /** DEMANDE la suppression d'une piste : ouvre une confirmation (jamais
     * de suppression directe — trop brutale). */
    const requestRemoveTrack = (channel) => {
        const t = tracks.find(tc => tc.channel === channel);
        if (!t)
            return;
        setPendingDeleteTrack(t);
    };
    /** Réordonne les pistes (drag & drop des lanes). L'ordre est partagé :
     * table de mixage, lanes et mode Live lisent tous le même tableau. */
    const reorderTracks = (from, to) => {
        setLocalTracks(prev => {
            if (from === to || from < 0 || to < 0 || from >= prev.length || to >= prev.length)
                return prev;
            const next = [...prev];
            const [moved] = next.splice(from, 1);
            next.splice(to, 0, moved);
            return next;
        });
    };
    /** Supprime réellement la piste (appelé après confirmation).
     * UI, moteur, notes du piano roll, backend. */
    const confirmRemoveTrack = () => {
        if (!pendingDeleteTrack)
            return;
        const channel = pendingDeleteTrack.channel;
        const t = pendingDeleteTrack;
        setLocalTracks(prev => prev.filter(tc => tc.channel !== channel));
        engineRef.current?.removeTrack(channel);
        setPianoNotes(prev => {
            const next = { ...prev };
            delete next[channel];
            return next;
        });
        setPendingDeleteTrack(null);
        setStatus(`🗑 Piste « ${t?.label ?? channel} » supprimée`);
        setStatusColor('text-blue-400');
    };
    // ── État : options musicales ─────────────────────────────────────
    const [loopOn, setLoopOn] = useState(false);
    // Locators [L, R[ (beats) — intervalle de boucle du repeat. R ≤ L =
    // pas d'intervalle (boucle complète du morceau).
    const [locL, setLocL] = useState(0);
    const [locR, setLocR] = useState(0);
    /** LivePiano cliquable (mode Live) : note au Roland, canal auto
     * (écho configuré, sinon 1). */
    const livePlayNote = useCallback((pitch, on) => {
        void sendPianoNote(pitch, on);
    }, []);
    const handleLocatorsChange = useCallback((l, r) => {
        setLocL(l);
        setLocR(r);
    }, []);
    const [walkingBass, setWalkingBass] = useState(false);
    const [drumPattern, setDrumPattern] = useState('rock');
    const [sig, setSig] = useState('4/4');
    // ── Boucle sample du mode Navig ──────────────────────────────────
    /** Sample audio (quelques mesures) répété en boucle pendant la lecture
     * Navig, joué par le navigateur (Web Audio) en parallèle du WAV principal.
     * `offsetMs` décale la phase EN DIRECT (vérification à l'oreille).
     * Persisté avec le projet (buildGrilleObject / autosave / Load). */
    const [sampleLoop, setSampleLoopState] = useState({
        enabled: false, sample: '', volume: DEFAULT_SAMPLE_VOLUME, offsetMs: 0,
    });
    /** Applique un changement de config (appliqué en direct au moteur Navig).
     * Le contexte grille (tempo + signature) est ajouté à l'appel moteur pour
     * que le sample soit RECADRÉ sur la mesure (coupe / silence) — jamais
     * persisté (le projet stocke déjà tempo et sig séparément). */
    const updateSampleLoop = (patch) => {
        setSampleLoopState(prev => {
            const next = { ...prev, ...patch };
            const beatsPerBar = parseInt((sig.split('/')[0] || '4'), 10) || 4;
            engineRef.current?.setSampleLoop(next.enabled && next.sample ? { ...next, tempo, beatsPerBar } : null);
            return next;
        });
    };
    const [status, setStatus] = useState('Prêt');
    const [statusColor, setStatusColor] = useState('text-gray-400');
    const [audioStarted, setAudioStarted] = useState(false);
    // ── Mode réel du clic (mode séparé) : channels / mixed_fallback ──
    useEffect(() => {
        const onClickMode = (e) => {
            const d = e.detail;
            if (d?.mode === 'mixed_fallback') {
                setStatus(`🥁 ${d.reason ?? 'Clic mélangé au son principal (sortie non multicanal)'}`);
                setStatusColor('text-amber-400');
            }
            else if (d?.mode === 'channels') {
                setStatus('🥁 Clic séparé : main canaux 1-2, clic 3-4 (synchro parfaite)');
                setStatusColor('text-emerald-400');
            }
        };
        window.addEventListener('chordzic:click-mode', onClickMode);
        return () => window.removeEventListener('chordzic:click-mode', onClickMode);
    }, []);
    // ── Auto-sauvegarde locale (anti-perte à l'actualisation) ──────────
    /** Horodatage (ms) du dernier autosave effectif — affiché dans le header. */
    const [lastAutosaveAt, setLastAutosaveAt] = useState(null);
    /** Dernier payload sérialisé — écrit SYNCHRONEMENT au beforeunload. */
    const autosavePayloadRef = useRef(null);
    // ── Suppression de piste : confirmation avant action ────────────────
    /** Piste en attente de confirmation de suppression (null = aucune). */
    const [pendingDeleteTrack, setPendingDeleteTrack] = useState(null);
    // ── État : piano roll (notes personnalisées par piste) ───────────
    const [pianoNotes, setPianoNotes] = useState({});
    const handlePianoRollChange = useCallback((channel, notes) => {
        setPianoNotes(prev => ({ ...prev, [channel]: notes }));
    }, []);
    // ── Dernier chiffrage tapé (pour l'autocomplétion de ChordInput) ──
    const [, setLastChiffrage] = useState('');
    // ── État : drag & drop ───────────────────────────────────────────
    const [dragIdx, setDragIdx] = useState(null);
    // ── État : modal détail ──────────────────────────────────────────
    const [selectedChord, setSelectedChord] = useState(null);
    const selectedChordIdx = useMemo(() => {
        if (!selectedChord || chords.length === 0)
            return -1;
        return chords.findIndex(c => c.time === selectedChord.time &&
            c.chiffrage === selectedChord.chiffrage &&
            c.name === selectedChord.name);
    }, [selectedChord, chords]);
    // ── État : sauvegarde / chargement ─────────────────────────────────
    const [showSaveModal, setShowSaveModal] = useState(false);
    const [showLoadModal, setShowLoadModal] = useState(false);
    const [showExportModal, setShowExportModal] = useState(false);
    /** Modal de confirmation « Nouveau projet » (action destructive). */
    const [showNewProjectModal, setShowNewProjectModal] = useState(false);
    /** Nom du projet courant (grille chargée/sauvegardée, sans extension) —
     * affiché à côté du titre. null = aucun projet nommé. */
    const [projectName, setProjectName] = useState(null);
    /** Documentation utilisateur (bouton ❓ du header). */
    const [showHelp, setShowHelp] = useState(false);
    const [savedGrilles, setSavedGrilles] = useState([]);
    const fileInputRef = useRef(null);
    // ── AudioEngine (instance unique) ─────────────────────────────────
    const engineRef = useRef(new AudioEngine());
    // ── PostProdEngine (instance unique — mode PostProd) ──────────────
    const ppEngineRef = useRef(new PostProdEngine());
    const getEngine = useCallback(async () => {
        if (!audioStarted) {
            await engineRef.current.init();
            setAudioStarted(true);
        }
        return engineRef.current;
    }, [audioStarted]);
    // ── Effet : initialisation au montage ─────────────────────────────
    useEffect(() => {
        getEngine();
        for (const t of tracks)
            engineRef.current.setTrack(t.channel, t);
        engineRef.current?.set432Hz(use432);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);
    // ── RESTAURATION automatique de la session (anti-perte à l'actualisation) ──
    // Au démarrage, si une auto-sauvegarde locale existe (même projet, même
    // navigateur), elle est restaurée : grille, tempo, mesure, pistes, notes
    // des piano rolls, pattern, 432 Hz. Plus rien n'est perdu en appuyant F5.
    useEffect(() => {
        try {
            const raw = localStorage.getItem(AUTOSAVE_KEY);
            if (!raw)
                return;
            const data = JSON.parse(raw);
            if (!data || data.type !== 'chordJAVA-grille')
                return;
            // ⚠️ NE PAS exiger data.input non vide : un projet mode Navig a une
            // grille vide (contenu dans les notes) — l'autosave doit se restaurer
            // quand même (bug « plus aucune persistance après F5 »).
            suppressAutoConfigRef.current = true;
            if (data.tempo)
                setTempo(data.tempo);
            if (data.sig)
                setSig(data.sig);
            setInput(data.input);
            setPianoNotes(data.pianoNotes ?? {});
            if (data.tracks && data.tracks.length > 0)
                applyLoadedTracks(data.tracks);
            if (data.pattern)
                setDrumPattern(data.pattern);
            if (data.use432Hz !== undefined)
                setUse432(data.use432Hz);
            if (data.loopOn !== undefined)
                setLoopOn(data.loopOn);
            if (data.walkingBass !== undefined)
                setWalkingBass(data.walkingBass);
            if (data.sampleLoop) {
                const raw = data.sampleLoop;
                const sl = {
                    enabled: raw.enabled ?? false,
                    sample: raw.sample ?? '',
                    volume: raw.volume ?? DEFAULT_SAMPLE_VOLUME,
                    offsetMs: raw.offsetMs ?? 0,
                };
                setSampleLoopState(sl);
                // Contexte grille chargé (tempo/sig locaux, pas encore dans le state)
                const bpb = parseInt(((data.sig || '4/4').split('/')[0] || '4'), 10) || 4;
                engineRef.current?.setSampleLoop(sl.enabled && sl.sample ? { ...sl, tempo: data.tempo || 120, beatsPerBar: bpb } : null);
            }
            setLastAutosaveAt(data.savedAt ? new Date(data.savedAt).getTime() : Date.now());
            setProjectName(typeof data.projectName === 'string' && data.projectName ? data.projectName : null);
            setStatus('♻️ Session restaurée (sauvegarde automatique locale)');
            setStatusColor('text-green-400');
            setTimeout(() => {
                try {
                    const grille = parseGrille(data.input, data.tempo || 120);
                    setChords(grille.chords);
                }
                catch { }
            }, 50);
        }
        catch {
            // Auto-sauvegarde absente ou corrompue → démarrage normal
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);
    // ── AUTO-SAUVEGARDE locale : debounce après chaque changement d'état ──
    // Toute modification (grille, tempo, pistes, notes, réglages) est écrite
    // dans localStorage quelques centaines de ms après le dernier changement.
    useEffect(() => {
        const t = setTimeout(() => {
            try {
                const payload = JSON.stringify(buildGrilleObject({ savedAt: new Date().toISOString(), autosave: true }));
                localStorage.setItem(AUTOSAVE_KEY, payload);
                autosavePayloadRef.current = payload;
                setLastAutosaveAt(Date.now());
            }
            catch { /* localStorage plein/indisponible : on continue sans autosave */ }
        }, AUTOSAVE_DEBOUNCE_MS);
        return () => clearTimeout(t);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [input, tempo, sig, tracks, pianoNotes, drumPattern, use432, loopOn, walkingBass, sampleLoop]);
    // ── FLUSH SYNCHRONE à la fermeture/actualisation : même si le debounce
    // n'a pas encore écrit, le dernier état connu est sauvegardé d'un coup. ──
    useEffect(() => {
        const handler = () => {
            try {
                const payload = autosavePayloadRef.current;
                if (payload)
                    localStorage.setItem(AUTOSAVE_KEY, payload);
            }
            catch { }
        };
        window.addEventListener('beforeunload', handler);
        return () => window.removeEventListener('beforeunload', handler);
    }, []);
    // ── Sauvegarde serveur : chargement + migration localStorage ─────
    const refreshGrilles = useCallback(async () => {
        try {
            const res = await fetch(`${API_BASE}/grilles`);
            if (res.ok)
                setSavedGrilles(await res.json());
        }
        catch { /* serveur injoignable : on garde l'état courant */ }
    }, []);
    useEffect(() => {
        // Migration unique : les anciennes grilles localStorage sont poussées
        // vers le serveur (format v3) puis retirées du localStorage.
        const migrateLocalStorage = async () => {
            let local = [];
            try {
                local = JSON.parse(localStorage.getItem(STORAGE_KEY) || '[]');
            }
            catch {
                local = [];
            }
            try {
                const res = await fetch(`${API_BASE}/grilles`);
                if (!res.ok)
                    throw new Error('serveur indisponible');
                const remote = await res.json();
                setSavedGrilles(remote);
                const remoteNames = new Set(remote.map(g => g.name));
                const toMigrate = local.filter(g => g.name && !remoteNames.has(g.name));
                for (const g of toMigrate) {
                    const hasPN = g.pianoNotes && Object.values(g.pianoNotes).some(n => n.length > 0);
                    await fetch(`${API_BASE}/save`, {
                        method: 'POST', headers: { 'Content-Type': 'application/json' },
                        body: JSON.stringify({
                            type: 'chordJAVA-grille', version: 3,
                            name: g.name, input: g.input, tempo: g.tempo, sig: g.sig,
                            savedAt: new Date().toISOString(),
                            ...(hasPN ? { pianoNotes: g.pianoNotes } : {}),
                        }),
                    });
                }
                if (toMigrate.length > 0) {
                    localStorage.removeItem(STORAGE_KEY);
                    await refreshGrilles();
                }
            }
            catch {
                // Serveur injoignable : repli sur les grilles locales (lecture seule)
                if (local.length)
                    setSavedGrilles(local);
            }
        };
        migrateLocalStorage();
    }, [refreshGrilles]);
    // ── Analyse de l'input ────────────────────────────────────────────
    const parseInput = () => {
        try {
            const grille = parseGrille(input, tempo);
            setChords(grille.chords);
            if (grille.chords.length > 0) {
                setLastChiffrage(grille.chords[grille.chords.length - 1].chiffrage);
            }
            setStatus(`✅ ${grille.chords.length} accords`);
            setStatusColor('text-green-400');
            setHighlighted(-1);
        }
        catch (e) {
            setStatus(`❌ ${e.message}`);
            setStatusColor('text-red-400');
        }
    };
    // ── Analyse automatique de l'input (debounce) ────────────────────
    // Plus besoin de cliquer « Analyser » : la grille est re-parsée
    // automatiquement 600 ms après la dernière modification de l'input
    // (et une fois au montage pour la grille par défaut).
    const lastParsedInput = useRef('');
    useEffect(() => {
        if (input === lastParsedInput.current)
            return;
        const t = setTimeout(() => {
            lastParsedInput.current = input;
            parseInput();
        }, 600);
        return () => clearTimeout(t);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [input]);
    // ─── Play / Stop / Clear ────────────────────────────────────────
    /** Bascule le mode Navigateur (WAV) / Live (MIDI). */
    const setNavigMode = (v) => {
        setBrowserAudio(v);
        engineRef.current.browserAudio = v;
    };
    // ── Pré-remplissage Navig : à l'activation du mode 📱, toutes les pistes
    // non encore initialisées reçoivent les notes du mode classique (seed).
    // Les pistes déjà remplies (éditées OU vidées par l'utilisateur) ne sont
    // jamais touchées. Re-déclenché quand la grille change ou quand les canaux
    // des pistes changent.
    const channelsKey = tracks.map(t => t.channel).join(',');
    useEffect(() => {
        if (!browserAudio || chords.length === 0)
            return;
        let cancelled = false;
        (async () => {
            try {
                const engine = await getEngine();
                const fetched = await engine.getPianoNotes({ titre: 'Session', tempo, chords });
                if (cancelled || !fetched)
                    return;
                setPianoNotes(prev => {
                    const next = { ...prev };
                    let changed = false;
                    for (const t of tracks) {
                        const ch = t.channel;
                        // Déjà initialisée (éditée ou vidée volontairement) → ne pas toucher
                        if (next[ch] !== undefined)
                            continue;
                        const chNotes = fetched
                            .filter(n => n.channel === ch)
                            .map((n, i) => ({
                            id: `seed-${ch}-${i}`,
                            startTime: n.start_time,
                            pitch: n.pitch,
                            duration: n.duration,
                            velocity: n.velocity,
                        }));
                        if (chNotes.length > 0) {
                            next[ch] = chNotes;
                            changed = true;
                        }
                    }
                    return changed ? next : prev;
                });
            }
            catch { /* silencieux */ }
        })();
        return () => { cancelled = true; };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [browserAudio, chords, tempo, channelsKey]);
    const playChordPreview = useCallback(async (chord) => {
        const engine = await getEngine();
        if (!engine)
            return;
        // Lookups par CANAL (les index fixes tracks[i] se décale si une piste
        // est supprimée — pistes dynamiques).
        const tLead = tracks.find(t => t.channel === 0);
        const tBass = tracks.find(t => t.channel === 2);
        const tStr = tracks.find(t => t.channel === 3);
        const tDrums = tracks.find(t => t.channel === 9);
        engine.setTrack(0, { program: tLead?.program ?? 0, mute: tLead?.mute ?? true });
        engine.setWalking(walkingBass);
        engine.set432Hz(use432);
        engine.setVolume(volume);
        engine.setDrums(!(tDrums?.mute ?? true));
        engine.setBass(!(tBass?.mute ?? true));
        engine.setArpeggios(!(tLead?.mute ?? true));
        engine.setNappes(!(tStr?.mute ?? true));
        engine.setPattern(drumPattern);
        engine.setSig(sig);
        engine.onHighlight((idx) => setHighlighted(idx));
        setPlaying(true);
        setStatus('▶ Prévisualisation...');
        setStatusColor('text-green-400');
        engine.playChordPreview(chord).then(() => {
            setPlaying(false);
            setHighlighted(-1);
            if (engineRef.current && !engineRef.current.isPlaying) {
                setStatus('✅ Arrêté');
                setStatusColor('text-gray-400');
            }
        }).catch((e) => {
            setPlaying(false);
            setStatus(`❌ Erreur: ${e.message}`);
            setStatusColor('text-red-400');
        });
    }, [tempo, volume, tracks, use432, drumPattern, sig, getEngine, walkingBass]);
    const play = useCallback(async (startAtBeats = 0, renderer) => {
        const engine = await getEngine();
        if (!engine)
            return;
        let chordsToPlay = chords;
        // Re-parser systématiquement si l'input a changé depuis le dernier
        // parse (debounce ou clic Play avant la fin du délai)
        if (input !== lastParsedInput.current) {
            lastParsedInput.current = input;
            try {
                const grille = parseGrille(input, tempo);
                chordsToPlay = grille.chords;
                setChords(grille.chords);
            }
            catch { }
        }
        // Convertir les pianoNotes en customNotes pour le backend (calculé
        // AVANT le garde-fou : la lecture Navig doit marcher avec des notes
        // même si la grille Live est vide).
        const customNotes = Object.entries(pianoNotes).flatMap(([ch, notes]) => notes.map(n => ({
            channel: parseInt(ch),
            start_time: n.startTime,
            pitch: n.pitch,
            duration: n.duration,
            velocity: n.velocity,
        })));
        // Rien à jouer : alerte UNIQUEMENT si la grille ET les notes sont vides.
        if (!hasPlayableContent(chordsToPlay.length, customNotes.length)) {
            setStatus('❌ Rien à jouer — entre des accords (Live) ou des notes (Navig)');
            setStatusColor('text-red-400');
            return;
        }
        // Lookups par CANAL (les index fixes tracks[i] se décale si une piste
        // est supprimée — pistes dynamiques).
        const tLead = tracks.find(t => t.channel === 0);
        const tBass = tracks.find(t => t.channel === 2);
        const tStr = tracks.find(t => t.channel === 3);
        const tDrums = tracks.find(t => t.channel === 9);
        engine.setTrack(0, { program: tLead?.program ?? 0, mute: tLead?.mute ?? true });
        engine.setWalking(walkingBass);
        engine.set432Hz(use432);
        engine.setVolume(volume);
        engine.setDrums(!(tDrums?.mute ?? true));
        engine.setBass(!(tBass?.mute ?? true));
        engine.setArpeggios(!(tLead?.mute ?? true));
        engine.setNappes(!(tStr?.mute ?? true));
        engine.setPattern(drumPattern);
        engine.setSig(sig);
        engine.onHighlight((idx) => setHighlighted(idx));
        setPlaying(true);
        setStatus('▶ Lecture...');
        setStatusColor('text-green-400');
        // Canaux en mode PianoRoll (ouverts/édités) — les autres canaux
        // continuent de jouer en mode classique
        const customChannels = Object.keys(pianoNotes).map(Number);
        const grille = { titre: 'Session', tempo, chords: chordsToPlay };
        // En mode Navig (rendu WAV), le WAV sera disponible à l'extraction
        if (browserAudio)
            setHasWav(true);
        engine.playGrille(grille, loopOn, customNotes.length > 0 ? customNotes : undefined, customChannels.length > 0 ? customChannels : undefined, locR > locL ? { start: locL, end: locR } : undefined, startAtBeats, renderer).then(() => {
            setPlaying(false);
            setHighlighted(-1);
            setStatus('✅ Lecture terminée');
            setStatusColor('text-green-400');
        }).catch((e) => {
            setPlaying(false);
            setStatus(`❌ Erreur: ${e.message}`);
            setStatusColor('text-red-400');
        });
    }, [chords, tempo, volume, tracks, use432, drumPattern, sig, getEngine, loopOn, input, browserAudio, pianoNotes, locL, locR]);
    const stop = () => {
        if (engineRef.current)
            engineRef.current.stop();
        setPlaying(false);
        setHighlighted(-1);
        setStatus('■ Arrêté');
        setStatusColor('text-gray-400');
    };
    const clear = () => { stop(); setChords([]); setHighlighted(-1); setStatus('Prêt'); setStatusColor('text-gray-400'); };
    // ─── Drag & Drop ────────────────────────────────────────────────
    const rebuildInputFromChords = (newChords) => {
        const newInput = newChords.map(c => `${c.time}:${c.chiffrage}`).join(' ');
        setInput(newInput);
        setChords(newChords);
        if (newChords.length > 0)
            setLastChiffrage(newChords[newChords.length - 1].chiffrage);
        setStatus('🔀 Grille réordonnée');
        setStatusColor('text-blue-400');
    };
    const handleDragStart = (idx) => setDragIdx(idx);
    const handleDragOver = (e) => { e.preventDefault(); e.dataTransfer.dropEffect = 'move'; };
    const handleDrop = (targetIdx) => {
        if (dragIdx === null || dragIdx === targetIdx) {
            setDragIdx(null);
            return;
        }
        const newChords = [...chords];
        const [moved] = newChords.splice(dragIdx, 1);
        newChords.splice(targetIdx, 0, moved);
        setDragIdx(null);
        rebuildInputFromChords(newChords);
    };
    // ─── Sauvegarder / Charger ─────────────────────────────────────
    /** Construit l'objet grille complet (format v3) — partagé save/export. */
    const buildGrilleObject = (extra) => {
        const hasPianoNotes = Object.keys(pianoNotes).length > 0 &&
            Object.values(pianoNotes).some(notes => notes.length > 0);
        return {
            type: 'chordJAVA-grille', version: 3, input, tempo, sig,
            tracks: tracks.map(t => ({ channel: t.channel, program: t.program, volume: t.volume, mute: t.mute, label: t.label, drums: t.drums ?? false, bank_msb: t.bankMsb ?? 0, bank_lsb: t.bankLsb ?? 0, fx: t.fx ?? FX_ZERO })),
            pattern: drumPattern, use432Hz: use432,
            loopOn, walkingBass,
            sampleLoop,
            projectName,
            ...(hasPianoNotes ? { pianoNotes } : {}),
            ...extra,
        };
    };
    const handleSave = async (saveName) => {
        const hasNotes = Object.values(pianoNotes).some(n => n.length > 0);
        // Mode Navig : l'input (grille texte) peut être vide — le contenu est
        // dans les notes des piano rolls (bug « sauvegarde impossible en Navig »).
        if (!saveName.trim() || !hasSaveableContent(input, hasNotes ? 1 : 0))
            return;
        const name = saveName.trim();
        try {
            const res = await fetch(`${API_BASE}/save`, {
                method: 'POST', headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(buildGrilleObject({ name, savedAt: new Date().toISOString() })),
            });
            if (!res.ok)
                throw new Error('échec serveur');
            await refreshGrilles();
            setShowSaveModal(false);
            setProjectName(name);
            setStatus(`💾 Grille « ${name} » sauvegardée (fichier JSON)`);
            setStatusColor('text-green-400');
        }
        catch {
            setStatus('❌ Sauvegarde impossible (serveur injoignable)');
            setStatusColor('text-red-400');
        }
    };
    const handleLoad = (entry) => {
        // Les instruments/paramètres sauvegardés ont priorité sur l'auto-config
        suppressAutoConfigRef.current = true;
        setInput(entry.input);
        setTempo(entry.tempo);
        setSig(entry.sig);
        // Restaurer les notes du PianoRoll (ancien format sans le champ → aucune)
        setPianoNotes(entry.pianoNotes ?? {});
        // v3 : restaurer aussi les réglages (tracks, pattern, 432Hz)
        // Les pistes de la grille REMPLACENT celles du projet courant (les
        // pistes orphelines de la session précédente sont retirées).
        if (entry.tracks && entry.tracks.length > 0)
            applyLoadedTracks(entry.tracks);
        if (entry.pattern)
            setDrumPattern(entry.pattern);
        if (entry.use432Hz !== undefined)
            setUse432(entry.use432Hz);
        if (entry.loopOn !== undefined)
            setLoopOn(entry.loopOn);
        if (entry.walkingBass !== undefined)
            setWalkingBass(entry.walkingBass);
        if (entry.sampleLoop) {
            const raw = entry.sampleLoop;
            const sl = {
                enabled: raw.enabled ?? false,
                sample: raw.sample ?? '',
                volume: raw.volume ?? DEFAULT_SAMPLE_VOLUME,
                offsetMs: raw.offsetMs ?? 0,
            };
            setSampleLoopState(sl);
            // Contexte grille chargée (tempo/sig de l'entrée, pas encore dans le state)
            const bpb = parseInt(((entry.sig || '4/4').split('/')[0] || '4'), 10) || 4;
            engineRef.current?.setSampleLoop(sl.enabled && sl.sample ? { ...sl, tempo: entry.tempo || 120, beatsPerBar: bpb } : null);
        }
        setProjectName(entry.name || null);
        setShowLoadModal(false);
        setStatus(`📂 Grille « ${entry.name} » chargée`);
        setStatusColor('text-blue-400');
        setTimeout(() => {
            try {
                const grille = parseGrille(entry.input, entry.tempo);
                setChords(grille.chords);
            }
            catch { }
        }, 50);
    };
    const handleDeleteSave = async (id) => {
        try {
            await fetch(`${API_BASE}/grilles/${encodeURIComponent(id)}`, { method: 'DELETE' });
            await refreshGrilles();
        }
        catch { /* silencieux */ }
    };
    // ─── Nouveau projet ─────────────────────────────────────────────
    /** Vrai si le projet courant ne contient rien (grille vide, aucune note
     * de piano roll, aucun nom) — dans ce cas pas besoin de confirmation. */
    const projectIsEmpty = input.trim() === '' &&
        Object.values(pianoNotes).every(notes => notes.length === 0) &&
        projectName === null;
    /** Demande la création d'un nouveau projet : confirmation si le projet
     * courant contient des données, sinon action directe. La lecture est
     * arrêtée dès l'ouverture (une modal + de la musique = brouhaha). */
    const requestNewProject = () => {
        stop();
        if (projectIsEmpty) {
            confirmNewProject();
            return;
        }
        setShowNewProjectModal(true);
    };
    /** Réinitialise COMPLÈTEMENT le projet : grille, tempo, mesure, pistes
     * (retour aux 5 par défaut, moteur synchronisé), notes des piano rolls,
     * réglages audio/musicaux, nom du projet, lecture et modals.
     * L'auto-sauvegarde locale est PURGÉE : un F5 après « Nouveau projet »
     * ne doit PAS restaurer l'ancien projet (le debounce réécrira ensuite
     * un autosave vierge puisque l'état a changé). */
    const confirmNewProject = () => {
        setShowNewProjectModal(false);
        stop();
        // Grille & lecture
        setInput('');
        setChords([]);
        setHighlighted(-1);
        setPlaying(false);
        setCurrentBeat(0);
        setSelectedChord(null);
        setLastChiffrage('');
        // Paramètres audio par défaut
        setTempo(120);
        setVolume(127);
        setUse432(false);
        engineRef.current?.set432Hz(false);
        // Pistes par défaut (remplace la liste ET synchronise le moteur)
        applyLoadedTracks(DEFAULT_TRACKS);
        // Piano rolls & édition
        setPianoNotes({});
        setDragIdx(null);
        // Options musicales
        setDrumPattern('rock');
        setLoopOn(false);
        setWalkingBass(false);
        // Boucle sample
        setSampleLoopState({ enabled: false, sample: '', volume: DEFAULT_SAMPLE_VOLUME, offsetMs: 0 });
        engineRef.current?.setSampleLoop(null);
        // Modes & sessions — on RESTE dans le mode courant (Live ou Navig) :
        // seul le contenu est remis à zéro.
        setHasWav(false);
        setPostProdSession(null);
        setShowPostProd(false);
        setBouncing(false);
        // Projet & modals
        setProjectName(null);
        setPendingDeleteTrack(null);
        setShowAddTrack(false);
        // Auto-sauvegarde : purge pour ne pas restaurer l'ancien projet au F5
        try {
            localStorage.removeItem(AUTOSAVE_KEY);
        }
        catch { /* localStorage indisponible */ }
        autosavePayloadRef.current = null;
        setLastAutosaveAt(null);
        setStatus('✨ Nouveau projet — repartez de zéro');
        setStatusColor('text-green-400');
    };
    const handleExport = () => setShowExportModal(true);
    const doExport = (name) => {
        const data = buildGrilleObject({ exportedAt: new Date().toISOString() });
        // Nom de fichier lisible : sanitisation du nom choisi par l'utilisateur
        const safeName = name.trim().replace(/[^\w\-]+/g, '_').replace(/_+/g, '_') || 'grille';
        const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = `${safeName}.json`;
        a.click();
        URL.revokeObjectURL(url);
        setShowExportModal(false);
        setStatus('📤 Grille exportée en JSON');
        setStatusColor('text-green-400');
    };
    /** Extrait le dernier rendu WAV (mode Navig) en fichier téléchargeable.
     * Si la boucle sample est active, le sample est MIXÉ au morceau avant
     * l'encodage (mêmes volume/offset que la lecture) — l'extraction reflète
     * exactement ce qu'on entend. */
    const handleExtractWav = async () => {
        const blob = await engineRef.current?.getExtractWavBlob();
        if (!blob) {
            setStatus('❌ Aucun WAV à extraire — lance une lecture d\'abord');
            setStatusColor('text-red-400');
            return;
        }
        // Nom de fichier : début de la grille + horodatage
        const base = (input.slice(0, 24).replace(/[^\w\-]+/g, '_').replace(/_+/g, '_').replace(/^_|_$/g, '')) || 'grille';
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = `${base}_${Date.now()}.wav`;
        a.click();
        URL.revokeObjectURL(url);
        const sampleInclus = sampleLoop.enabled && !!sampleLoop.sample;
        setStatus(`📥 WAV extrait (${(blob.size / 1048576).toFixed(1)} Mo${sampleInclus ? ' · sample inclus' : ''})`);
        setStatusColor('text-green-400');
    };
    /** Lecture MIDI (mode Navig) : joue TOUTES les pistes (grille + notes
     * personnalisées) sur le port MIDI choisi (ex. Roland) — comme le mode Live.
     * `startAtBeats` : position de départ (0 = début). */
    const playMidiAll = useCallback(async (startAtBeats = 0, excludeChannel) => {
        const hasNotes = Object.values(pianoNotes).some(notes => notes.length > 0);
        if (!hasPlayableContent(chords.length, hasNotes ? 1 : 0)) {
            setStatus('❌ Rien à jouer — entre des accords (Live) ou des notes (Navig)');
            setStatusColor('text-red-400');
            return;
        }
        const sequence = chords.map(c => ({ notes: chordToNoteNames(c), beats: 4.0 / c.time }));
        const customNotes = Object.entries(pianoNotes).flatMap(([ch, notes]) => notes.map(n => ({
            channel: parseInt(ch), start_time: n.startTime, pitch: n.pitch,
            duration: n.duration, velocity: n.velocity,
        })));
        const customChannels = Object.keys(pianoNotes).map(Number);
        const body = {
            sequence, tempo, pattern: drumPattern, walking: walkingBass, sig,
            loop_enabled: loopOn,
            tracks: tracks.map(t => ({
                channel: t.channel, program: t.program, volume: t.volume, mute: t.mute,
                drums: t.drums ?? false, bank_msb: t.bankMsb ?? 0, bank_lsb: t.bankLsb ?? 0, effects: t.fx ?? FX_ZERO,
            })),
            master_vol: volume,
            custom_notes: customNotes.length > 0 ? customNotes : undefined,
            custom_channels: customChannels.length > 0 ? customChannels : undefined,
            start_at: startAtBeats > 0 ? startAtBeats : undefined,
            // Play-along REC : le canal en cours d'enregistrement est exclu
            // (l'utilisateur joue cette piste lui-même — les autres accompagnent).
            ...(excludeChannel !== undefined ? { exclude_channel: excludeChannel } : {}),
            // Locators [L, R[ : le repeat boucle l'intervalle au lieu du morceau
            // complet (le backend ne les utilise que si loop_enabled).
            ...(locR > locL ? { loop_start: locL, loop_end: locR } : {}),
        };
        try {
            const resp = await fetch(`${API_BASE}/navig-play-midi`, {
                method: 'POST', headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(body),
            });
            if (!resp.ok) {
                const msg = await resp.text();
                setStatus(`❌ MIDI : ${msg.slice(0, 120)}`);
                setStatusColor('text-red-400');
                return;
            }
            const data = await resp.json();
            setStatus(`🎹 MIDI : ${data.notes} notes sur le port choisi (${data.duration_sec}s)`);
            setStatusColor('text-green-400');
        }
        catch (e) {
            setStatus('❌ MIDI : backend injoignable');
            setStatusColor('text-red-400');
        }
    }, [chords, pianoNotes, tempo, drumPattern, walkingBass, sig, tracks, volume, loopOn, locL, locR]);
    /** Bounce multitrack → mode PostProd.
     * Rend chaque piste en WAV (avec ses effets MIDI) via /render-tracks, décode
     * les buffers et construit la session d'édition audio (1 clip plein par piste). */
    const bounceToPostProd = async () => {
        if (chords.length === 0) {
            setStatus('❌ Rien à bouncer — grille vide');
            setStatusColor('text-red-400');
            return;
        }
        stop();
        setBouncing(true);
        setStatus('⏳ Bounce multitrack…');
        setStatusColor('text-blue-400');
        try {
            const sequence = chords.map(c => ({ notes: chordToNoteNames(c), beats: 4.0 / c.time }));
            const customNotes = Object.entries(pianoNotes).flatMap(([ch, notes]) => notes.map(n => ({
                channel: parseInt(ch), start_time: n.startTime, pitch: n.pitch,
                duration: n.duration, velocity: n.velocity,
            })));
            const customChannels = Object.keys(pianoNotes).map(Number);
            const body = {
                sequence, tempo, pattern: drumPattern, walking: walkingBass, sig,
                tracks: tracks.map(t => ({
                    channel: t.channel, program: t.program, volume: t.volume, mute: t.mute,
                    drums: t.drums ?? false, bank_msb: t.bankMsb ?? 0, bank_lsb: t.bankLsb ?? 0, effects: t.fx ?? FX_ZERO,
                })),
                master_vol: volume,
                custom_notes: customNotes.length > 0 ? customNotes : undefined,
                custom_channels: customChannels.length > 0 ? customChannels : undefined,
            };
            const resp = await fetch(`${API_BASE}/render-tracks`, {
                method: 'POST', headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(body),
            });
            if (!resp.ok)
                throw new Error(`render-tracks ${resp.status}`);
            const data = await resp.json();
            // Construire la session dans l'ORDRE des pistes du projet (non mutées)
            const sessionTracks = [];
            const bounced = (data.tracks ?? []);
            for (const t of tracks) {
                const ft = bounced.find(x => x.channel === t.channel);
                if (!ft)
                    continue; // piste mutée → absente du bounce
                const buffer = await ppEngineRef.current.decodeWav(ft.url);
                const duration = Math.min(buffer.duration, data.duration_sec);
                sessionTracks.push({
                    channel: ft.channel,
                    label: t.label,
                    program: ft.program,
                    color: trackColorForChannel(ft.channel),
                    buffer,
                    volume: 1.0, // fader neutre : le masterGain reproduit le niveau Navig
                    pan: 0,
                    mute: false,
                    solo: false,
                    clips: [createFullClip(ft.channel, duration)],
                });
            }
            // Les pistes AUDIO importées de la session précédente sont conservées
            // (le re-bounce ne remplace que les pistes MIDI) — même comportement
            // que les DAW : le re-render des instruments ne touche pas à l'audio.
            const imported = postProdSession?.tracks.filter(t => t.source === 'import') ?? [];
            // La durée de la timeline couvre aussi les clips importés (jamais tronqués)
            let totalDur = data.duration_sec;
            for (const t of imported)
                for (const c of t.clips)
                    totalDur = Math.max(totalDur, c.start + c.duration);
            const session = {
                projectName: projectName ?? 'projet',
                tempo: data.tempo,
                sig: data.sig,
                durationSec: totalDur,
                masterGain: data.master_gain,
                tracks: [...sessionTracks, ...imported],
            };
            ppEngineRef.current.loadSession(session);
            setPostProdSession(session);
            setShowPostProd(true);
            setStatus(`✅ Bounce → PostProd : ${sessionTracks.length} piste${sessionTracks.length > 1 ? 's' : ''} audio`);
            setStatusColor('text-green-400');
        }
        catch (e) {
            setStatus(`❌ Bounce impossible : ${e.message}`);
            setStatusColor('text-red-400');
        }
        finally {
            setBouncing(false);
        }
    };
    const handleImport = (e) => {
        const file = e.target.files?.[0];
        if (!file)
            return;
        const reader = new FileReader();
        reader.onload = (ev) => {
            try {
                const data = JSON.parse(ev.target?.result);
                if (data.type === 'chordJAVA-grille' && data.input) {
                    // Les instruments/paramètres importés ont priorité sur l'auto-config
                    suppressAutoConfigRef.current = true;
                    setInput(data.input);
                    setTempo(data.tempo || 120);
                    if (data.sig)
                        setSig(data.sig);
                    if (data.tracks && data.tracks.length > 0)
                        applyLoadedTracks(data.tracks);
                    else {
                        if (data.drums !== undefined)
                            updateTrack(9, { mute: !data.drums });
                        if (data.bass !== undefined)
                            updateTrack(2, { mute: !data.bass });
                        if (data.arpeggios !== undefined)
                            updateTrack(0, { mute: !data.arpeggios });
                        if (data.nappes !== undefined)
                            updateTrack(3, { mute: !data.nappes });
                        if (data.instrument !== undefined)
                            updateTrack(0, { program: data.instrument });
                    }
                    if (data.pattern)
                        setDrumPattern(data.pattern);
                    if (data.use432Hz !== undefined)
                        setUse432(data.use432Hz);
                    if (data.loopOn !== undefined)
                        setLoopOn(data.loopOn);
                    if (data.walkingBass !== undefined)
                        setWalkingBass(data.walkingBass);
                    if (data.version >= 3 && data.pianoNotes) {
                        setPianoNotes(data.pianoNotes);
                    }
                    else {
                        setPianoNotes({});
                    }
                    setStatus(`📥 Grille importée depuis ${file.name}`);
                    setStatusColor('text-green-400');
                    setProjectName(file.name.replace(/\.json$/i, '').trim() || null);
                    setTimeout(() => { try {
                        const grille = parseGrille(data.input, data.tempo || 120);
                        setChords(grille.chords);
                    }
                    catch { } }, 50);
                }
                else {
                    setStatus('❌ Format de fichier invalide');
                    setStatusColor('text-red-400');
                }
            }
            catch {
                setStatus('❌ Fichier JSON invalide');
                setStatusColor('text-red-400');
            }
        };
        reader.readAsText(file);
        e.target.value = '';
    };
    // ─── Effets divers ──────────────────────────────────────────────
    // ── Répercussion des mutes de l'UI vers le moteur ────────────────
    // Dépendances par VALEURS SÛRES (lookup par canal + fallback true) :
    // les anciennes dépendances tracks[i].mute plantaient quand le tableau
    // devenait plus court qu'un rôle supprimé.
    const mute9 = tracks.find(t => t.channel === 9)?.mute ?? true;
    const mute2 = tracks.find(t => t.channel === 2)?.mute ?? true;
    const mute0 = tracks.find(t => t.channel === 0)?.mute ?? true;
    const mute3 = tracks.find(t => t.channel === 3)?.mute ?? true;
    useEffect(() => { engineRef.current?.setDrums(!mute9); }, [mute9]);
    useEffect(() => { engineRef.current?.setBass(!mute2); }, [mute2]);
    useEffect(() => { engineRef.current?.setArpeggios(!mute0); }, [mute0]);
    useEffect(() => { engineRef.current?.setNappes(!mute3); }, [mute3]);
    useEffect(() => {
        // Auto-config Reggae : quand on sélectionne le pattern reggae, les
        // paramètres suivants sont forcés automatiquement. Suspendue pendant
        // un chargement de grille (les instruments sauvegardés ont priorité).
        if (suppressAutoConfigRef.current) {
            suppressAutoConfigRef.current = false;
            engineRef.current?.setPattern(drumPattern);
            return;
        }
        if (drumPattern === 'reggae') {
            updateTrack(0, { program: 16, volume: 114 }); // Lead → Drawbar Organ
            updateTrack(4, { program: 4, volume: 114 }); // Accent → Electric Piano 1
            updateTrack(2, { program: 32, volume: 109 }); // Bass → Acoustic Bass
            updateTrack(9, { volume: 127 }); // Drums → vol max
            updateTrack(3, { program: 0, volume: 80, mute: false }); // Nappes → Acoustic Grand Piano (joue seulement sur accords courts)
            setLoopOn(true); // Loop activé
        }
        engineRef.current?.setPattern(drumPattern);
    }, [drumPattern]);
    useEffect(() => { engineRef.current?.setWalking(walkingBass); }, [walkingBass]);
    useEffect(() => {
        if (!playing) {
            setCurrentBeat(0);
            return;
        }
        const msPerBeat = 60000 / tempo;
        setCurrentBeat(0);
        const interval = setInterval(() => setCurrentBeat(prev => (prev + 1) % 4), msPerBeat);
        return () => clearInterval(interval);
    }, [playing, tempo]);
    useEffect(() => {
        fetch('http://localhost:4000/config', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ sig }) }).catch(() => { });
        engineRef.current?.setSig(sig);
    }, [sig]);
    useEffect(() => {
        fetch('http://localhost:4000/config', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ tempo }) }).catch(() => { });
        engineRef.current?.setTempo(tempo);
    }, [tempo]);
    // RECADRAGE de la boucle sample au changement de tempo/signature : la
    // période de boucle (multiple entier de la mesure) dépend des deux — on
    // rappelle setSampleLoop pour que le moteur recalcule le buffer recadré,
    // même pendant la lecture (le sample reste synchrone du métronome).
    useEffect(() => {
        if (!sampleLoop.enabled || !sampleLoop.sample)
            return;
        const beatsPerBar = parseInt((sig.split('/')[0] || '4'), 10) || 4;
        engineRef.current?.setSampleLoop({
            ...sampleLoop, tempo, beatsPerBar,
        });
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [tempo, sig]);
    // Parse automatique avec debounce
    const debounceRef = useRef(undefined);
    useEffect(() => {
        if (debounceRef.current)
            clearTimeout(debounceRef.current);
        debounceRef.current = setTimeout(() => {
            if (!input.trim()) {
                setChords([]);
                setStatus('Prêt');
                setStatusColor('text-gray-400');
                return;
            }
            try {
                const grille = parseGrille(input, tempo);
                setChords(grille.chords);
            }
            catch {
                setChords([]);
            }
        }, 300);
        return () => { if (debounceRef.current)
            clearTimeout(debounceRef.current); };
    }, [input, tempo]);
    const handleUpdateChord = useCallback((idx, newText) => {
        // Réécriture complète depuis la liste des accords (et non remplacement
        // du token brut) : avec la notation de répétition « xN », l'index de la
        // liste ne correspond plus à l'index des tokens du texte — éditer une
        // occurrence éclate proprement le xN en forme longue.
        try {
            const updated = parseChord(newText);
            const newChords = [...chords];
            if (idx >= 0 && idx < newChords.length)
                newChords[idx] = updated;
            setChords(newChords);
            setInput(newChords.map(c => `${c.time}:${c.chiffrage}`).join(' '));
            setLastChiffrage(newChords[newChords.length - 1]?.chiffrage ?? '');
            if (newChords.length > 0 && idx < newChords.length)
                setSelectedChord(newChords[idx]);
        }
        catch { /* token invalide : on ignore */ }
    }, [chords]);
    /** Insère l'accord détecté par le ChordDetector à la fin de la grille
     * (durée par défaut : 1 temps = ronde — une grille construite en jouant
     * s'écrit en rondes ; éditable ensuite dans la grille).
     * Fonction stable (aucune dépendance) : le parse debounce existant
     * (300 ms sur `input`) met à jour la grille automatiquement. */
    const insertDetectedChord = useCallback((label) => {
        const token = `1:${label}`;
        setInput(prev => {
            const cur = prev.trim();
            return cur ? `${cur} ${token}` : token;
        });
    }, []);
    // ─── Rendu JSX ─────────────────────────────────────────────────────
    return (_jsx("div", { className: "min-h-screen bg-gray-950 flex items-start justify-center px-4 sm:px-6 py-4", children: _jsxs("div", { className: "w-full mx-auto", children: [_jsxs("div", { className: "flex items-center justify-between gap-3 mb-6 flex-wrap", children: [_jsxs("div", { className: "flex items-center gap-3", children: [_jsx("div", { className: "w-10 h-10 bg-blue-600 rounded-xl flex items-center justify-center", children: _jsx(Music, { className: "w-5 h-5 text-white" }) }), _jsxs("div", { children: [_jsxs("h1", { className: "text-xl font-bold text-white", children: ["chordZic", projectName ? _jsxs("span", { className: "text-gray-400 font-normal", children: [" \u2014 ", projectName] }) : null] }), _jsx("p", { className: "text-xs text-gray-500", children: "Moteur Harmonique - by Legoeland" })] })] }), _jsxs("div", { className: "flex items-center gap-2", children: [_jsx("span", { className: `text-xs font-mono ${statusColor}`, children: status }), lastAutosaveAt !== null && (_jsxs("span", { className: "text-[10px] text-emerald-500/90 font-mono shrink-0", title: "Auto-sauvegarde locale active : la session est restaur\u00E9e automatiquement en cas d'actualisation ou de fermeture (localStorage).", children: ["\uD83D\uDCBE ", new Date(lastAutosaveAt).toLocaleTimeString('fr-FR', { hour: '2-digit', minute: '2-digit' })] })), _jsx("button", { onClick: () => setShowHelp(true), className: "w-8 h-8 rounded-lg bg-gray-800 text-gray-400 border border-gray-700 hover:text-yellow-300 hover:border-gray-500 transition-colors text-sm font-bold shrink-0", title: "Aide \u2014 documentation utilisateur", children: "\u2753" })] })] }), postProdSession && showPostProd ? (_jsx(PostProdView, { session: postProdSession, engine: ppEngineRef.current, projectName: projectName, onBackToNavig: () => setShowPostProd(false), onSessionChange: setPostProdSession, onStatus: (msg, color = 'text-gray-400') => { setStatus(msg); setStatusColor(color); } })) : browserAudio ? (_jsx(DawView, { tracks: tracks, pianoNotes: pianoNotes, playing: playing, hasWav: hasWav, tempo: tempo, loopOn: loopOn, locL: locL, locR: locR, onLocatorsChange: handleLocatorsChange, onPlay: play, onExtractWav: handleExtractWav, onTempoChange: setTempo, onSetLoop: setLoopOn, onSetLive: () => setNavigMode(false), onSave: () => setShowSaveModal(true), onLoad: () => setShowLoadModal(true), onExport: handleExport, onImport: () => fileInputRef.current?.click(), onNewProject: requestNewProject, onAddTrack: () => setShowAddTrack(true), sampleLoop: sampleLoop, onSampleLoopChange: updateSampleLoop, onRemoveTrack: requestRemoveTrack, onUpdateTrack: updateTrack, onReorderTracks: reorderTracks, onNotesChange: handlePianoRollChange, onPlayMidiAll: playMidiAll, onHelp: () => setShowHelp(true), engine: engineRef.current, input: input, sig: sig, volume: volume, onSetVolume: setVolume, use432: use432, onSet432: setUse432, walkingBass: walkingBass, onSetWalkingBass: setWalkingBass, drumPattern: drumPattern, onSetDrumPattern: setDrumPattern, onSetSig: setSig, onPostProd: bounceToPostProd, bouncing: bouncing })) : (_jsxs(_Fragment, { children: [_jsx(ChordInput, { input: input, onChange: setInput }), _jsx(PianoLivePanel, { mode: "live", onInsert: (chord) => insertDetectedChord(chord.label), onGoNavig: () => setNavigMode(true), onPlayNote: livePlayNote }), _jsx("div", { className: "bg-gray-900 rounded-xl border border-gray-800 p-2 sm:p-3 mb-2 overflow-x-auto", children: _jsx(ControlBar, { chords: chords, playing: playing, onAnalyse: parseInput, onPlay: play, onStop: stop, onClear: clear, onSave: () => setShowSaveModal(true), onLoad: () => setShowLoadModal(true), onExport: handleExport, onImport: () => fileInputRef.current?.click(), onNewProject: requestNewProject, onExtractWav: handleExtractWav, hasWav: hasWav }) }), _jsx("div", { className: "bg-gray-900/60 rounded-lg border border-gray-800/80 px-3 py-2 mb-2", children: _jsx(LiveSettingsBar, { volume: volume, onSetVolume: setVolume, use432: use432, onSet432: setUse432, loopOn: loopOn, onSetLoop: setLoopOn, walkingBass: walkingBass, onSetWalkingBass: setWalkingBass, drumPattern: drumPattern, onSetDrumPattern: setDrumPattern, sig: sig, onSetSig: setSig, playing: playing, tempo: tempo, onTempoChange: setTempo }) }), _jsx(ProgressBar, { chords: chords, highlighted: highlighted, playing: playing, currentBeat: currentBeat, tempo: tempo }), _jsx(ChordGrid, { chords: chords, highlighted: highlighted, playing: playing, dragIdx: dragIdx, tempo: tempo, onClickChord: setSelectedChord, onDragStart: handleDragStart, onDragOver: handleDragOver, onDrop: handleDrop, onDragEnd: () => setDragIdx(null), onDeleteChord: (idx) => {
                                const newChords = chords.filter((_, i) => i !== idx);
                                newChords.length === 0 ? clear() : rebuildInputFromChords(newChords);
                            } }), chords.length === 0 && (_jsxs("div", { className: "bg-gray-900 rounded-xl border border-gray-800 p-12 text-center", children: [_jsx(Sparkles, { className: "w-12 h-12 text-gray-700 mx-auto mb-4" }), _jsx("p", { className: "text-gray-500 text-sm", children: "Entre des accords pour commencer" })] })), _jsx(ChordNowModal, { chords: chords, highlighted: highlighted, playing: playing })] })), _jsx("input", { ref: fileInputRef, type: "file", accept: ".json", onChange: handleImport, className: "hidden" }), _jsx(SaveModal, { show: showSaveModal, onClose: () => setShowSaveModal(false), onSave: handleSave }), _jsx(SaveModal, { show: showExportModal, onClose: () => setShowExportModal(false), onSave: doExport, title: "\uD83D\uDCE4 Exporter la grille en JSON", placeholder: "Nom du fichier", buttonLabel: "Exporter" }), _jsx(LoadModal, { show: showLoadModal, onClose: () => setShowLoadModal(false), grilles: savedGrilles, onLoad: handleLoad, onDelete: handleDeleteSave }), _jsx(NewProjectModal, { show: showNewProjectModal, onClose: () => setShowNewProjectModal(false), onConfirm: confirmNewProject }), showAddTrack && (_jsx("div", { className: "fixed inset-0 z-[70] flex items-center justify-center bg-black/60", children: _jsxs("div", { className: "bg-gray-900 rounded-xl border border-gray-700 shadow-2xl max-w-md w-full mx-4 p-5", children: [_jsxs("div", { className: "flex items-center gap-2 mb-4", children: [_jsx("span", { className: "text-xl", children: "\u2795" }), _jsx("h3", { className: "text-white font-bold", children: "Ajouter une piste" })] }), _jsxs("div", { className: "grid gap-2 mb-4", children: [_jsxs("button", { onClick: () => addTrack('inst'), className: "flex items-center gap-3 px-4 py-3 rounded-lg bg-gray-800 border border-gray-700 hover:border-blue-500 hover:bg-gray-750 text-left transition-colors", children: [_jsx("span", { className: "text-2xl", children: "\uD83C\uDFB9" }), _jsxs("span", { children: [_jsx("span", { className: "block text-white text-sm font-bold", children: "Piste instrument" }), _jsx("span", { className: "block text-xs text-gray-500", children: "Instrument GM au choix (128), notes m\u00E9lodiques" })] })] }), _jsxs("button", { onClick: () => addTrack('drums'), className: "flex items-center gap-3 px-4 py-3 rounded-lg bg-gray-800 border border-gray-700 hover:border-red-400 hover:bg-gray-750 text-left transition-colors", children: [_jsx("span", { className: "text-2xl", children: "\uD83E\uDD41" }), _jsxs("span", { children: [_jsx("span", { className: "block text-white text-sm font-bold", children: "Piste drums / percussion" }), _jsx("span", { className: "block text-xs text-gray-500", children: "Kit de percussion GM \u2014 canal 9 s'il est libre, sinon canal libre (banque percussion)" })] })] })] }), _jsx("div", { className: "flex justify-end", children: _jsx("button", { onClick: () => setShowAddTrack(false), className: "px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 text-sm", children: "Annuler" }) })] }) })), pendingDeleteTrack && (() => {
                    const t = pendingDeleteTrack;
                    const noteCount = (pianoNotes[t.channel] ?? []).length;
                    return (_jsx("div", { className: "fixed inset-0 z-[70] flex items-center justify-center bg-black/60", children: _jsxs("div", { className: "bg-gray-900 rounded-xl border border-red-800/70 shadow-2xl max-w-md w-full mx-4 p-5", children: [_jsxs("div", { className: "flex items-center gap-2 mb-3", children: [_jsx("span", { className: "text-xl", children: '\ud83d\uddd1\ufe0f' }), _jsxs("h3", { className: "text-white font-bold", children: ["Supprimer la piste \u00AB ", t.label, " \u00BB ?"] })] }), _jsxs("p", { className: "text-gray-300 text-sm leading-relaxed mb-4", children: ["Canal ", _jsx("b", { className: "text-white", children: t.channel }), noteCount > 0 && (_jsxs(_Fragment, { children: [" \u2014 ", _jsxs("b", { className: "text-red-400", children: [noteCount, " note", noteCount > 1 ? 's' : ''] }), " dans son piano roll"] })), ' ', "seront d\u00E9finitivement supprim\u00E9s. Cette action est irr\u00E9versible."] }), _jsxs("div", { className: "flex justify-end gap-2", children: [_jsx("button", { onClick: () => setPendingDeleteTrack(null), className: "px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 text-sm", children: "Annuler" }), _jsxs("button", { onClick: confirmRemoveTrack, className: "px-4 py-2 rounded-lg bg-red-700 text-white hover:bg-red-600 text-sm font-bold", children: ['\ud83d\uddd1\ufe0f', " Supprimer"] })] })] }) }));
                })(), _jsx(ChordDetailModal, { chord: selectedChord, chordIdx: selectedChordIdx, chordsCount: chords.length, playing: () => playing, onClose: () => setSelectedChord(null), onTogglePlay: () => { playing ? stop() : selectedChord && playChordPreview(selectedChord); }, onPrev: () => {
                        const n = chords[selectedChordIdx - 1];
                        if (n) {
                            setSelectedChord(n);
                            if (playing)
                                playChordPreview(n);
                        }
                    }, onNext: () => {
                        const n = chords[selectedChordIdx + 1];
                        if (n) {
                            setSelectedChord(n);
                            if (playing)
                                playChordPreview(n);
                        }
                    }, onUpdateChord: handleUpdateChord }), _jsx(HelpModal, { show: showHelp, onClose: () => setShowHelp(false) }), _jsxs("div", { className: "text-center mt-4 text-[10px] text-gray-700", children: ["chordJAVA v2 by Legoeland \u00B7 Render WAV \u00B7 ", AudioEngine.INSTRUMENTS.length, " instruments \u00B7 ", use432 ? 'A=432Hz' : 'A=440Hz'] })] }) }));
}
