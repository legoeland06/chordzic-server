import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 🎹 PianoLivePanel — panneau commun des deux modes (Live et Navig).
 *
 * Il embarque le piano Live (LivePiano), la reconnaissance d'accords en
 * temps réel (poll /live-input, le Roland) et l'insertion :
 * - mode `live`  → l'accord reconnu s'insère dans la GRILLE (1 ronde) ;
 * - mode `navig` → l'accord reconnu s'insère en NOTES dans le piano roll
 *   de la piste sélectionnée en amont (celle dont la lane est agrandie),
 *   converti par `chordToPianoNotes`.
 *
 * Illumination du piano :
 * - mode `live`  : les touches tenues sur le Roland s'illument ;
 * - mode `navig` : les touches s'illument au contenu de la piste jouée
 *   (trackPitches, quel que soit le mode de lecture wav/midi) — activable /
 *   désactivable par l'utilisateur (toggle ✨).
 *
 * Insertion : clic sur l'accord (ou « + Grille » / « ➕ Piste ») immédiat,
 * ou ⏱ timer indépendant — un accord identifié tenu ≥ 3 s (réglable) est
 * inséré automatiquement (les deux mains sont occupées à jouer).
 */
import { memo, useEffect, useRef, useState } from 'react';
import { recognizeChord } from '../lib/chordRecognition';
import { computeAutoInsert, initialAutoInsertState } from '../lib/autoInsert';
import { NOTE_NAMES } from '../types/chord';
import { activePitchesAt } from '../lib/pitchesToNotes';
import { getPlayheadPosition } from '../lib/playhead';
import LivePiano from './LivePiano';
import { backendUrl } from '../lib/chordUtils';
const API_BASE = backendUrl();
const POLL_MS = 150;
const AUTO_INSERT_DELAYS = [1, 2, 3, 5];
/** Vrai si deux listes de pitchs sont identiques (évite les re-renders). */
function samePitches(a, b) {
    return a.length === b.length && a.every((v, i) => v === b[i]);
}
/** Illumination de la piste jouée : lit la position du store playhead à
 * ~12 fps (imperceptible) et calcule les pitchs actifs — le DAW ne re-rend
 * plus à chaque tick de lecture (optimisation performance B). */
function usePlayheadPitches(notes, enabled, fps = 12) {
    const [pitches, setPitches] = useState([]);
    useEffect(() => {
        if (!enabled) {
            setPitches([]);
            return;
        }
        const update = () => setPitches(activePitchesAt(notes, getPlayheadPosition()));
        update();
        const id = setInterval(update, Math.max(40, Math.round(1000 / fps)));
        return () => clearInterval(id);
    }, [notes, enabled, fps]);
    return pitches;
}
function PianoLivePanel({ mode, onInsert, targetTrackLabel = null, trackNotes = [], illuminationEnabled = true, onToggleIllumination, onGoNavig, onGoLive, onPlayNote, }) {
    const [device, setDevice] = useState(null);
    const [detected, setDetected] = useState(null);
    const [active, setActive] = useState([]);
    const [delayS, setDelayS] = useState(3);
    const [justInserted, setJustInserted] = useState(false);
    // État du timer d'insertion automatique (persisté entre les ticks).
    const timerRef = useRef(initialAutoInsertState());
    const flashRef = useRef(undefined);
    useEffect(() => {
        let cancelled = false;
        const tick = async () => {
            try {
                const res = await fetch(`${API_BASE}/live-input`);
                if (!res.ok)
                    throw new Error(`HTTP ${res.status}`);
                const j = await res.json();
                if (cancelled)
                    return;
                setDevice(j.device ?? null);
                const pitches = Array.isArray(j.active) ? j.active : [];
                setActive(prev => (samePitches(prev, pitches) ? prev : pitches));
                const r = recognizeChord(pitches);
                setDetected(r);
                // ── Timer d'insertion automatique (mode live : grille ; navig :
                //    piste sélectionnée — pas d'insertion sans piste cible) ──
                const noTrack = mode === 'navig' && !targetTrackLabel;
                const insertable = (r?.insertable ?? false) && !noTrack;
                const key = r ? `${r.label}|${r.classes.join(',')}` : null;
                const verdict = computeAutoInsert(timerRef.current, Date.now(), delayS * 1000, key, insertable);
                timerRef.current = verdict.next;
                if (verdict.shouldInsert && r) {
                    onInsert(r, pitches);
                    setJustInserted(true);
                    if (flashRef.current)
                        clearTimeout(flashRef.current);
                    flashRef.current = setTimeout(() => setJustInserted(false), 1200);
                }
            }
            catch {
                if (!cancelled) {
                    setDevice(null);
                    setDetected(null);
                }
            }
        };
        tick();
        const id = setInterval(tick, POLL_MS);
        return () => {
            cancelled = true;
            clearInterval(id);
            if (flashRef.current)
                clearTimeout(flashRef.current);
        };
    }, [delayS, onInsert, mode, targetTrackLabel]);
    const canInsert = detected !== null && detected.insertable;
    const noKeyboard = device === null;
    const noTrack = mode === 'navig' && !targetTrackLabel;
    const insertDisabled = !canInsert || noTrack;
    // Illumination : Live = Roland tenu · Navig = Roland tenu (comme Live)
    // + piste jouée (toggle ✨) — les pitchs actifs de la piste viennent du
    // store playhead (~12 fps), pas d'un state du DAW.
    const trackPitches = usePlayheadPitches(trackNotes, illuminationEnabled && mode === 'navig');
    const pianoPitches = mode === 'live'
        ? active
        : [...new Set([...active, ...trackPitches])];
    const cycleDelay = () => {
        setDelayS(prev => {
            const i = AUTO_INSERT_DELAYS.indexOf(prev);
            return AUTO_INSERT_DELAYS[(i + 1) % AUTO_INSERT_DELAYS.length];
        });
    };
    const noteNames = detected && detected.classes.length > 0
        ? detected.classes.map(c => NOTE_NAMES[c]).join(' · ')
        : '';
    const handleInsert = () => {
        if (insertDisabled || !detected)
            return;
        onInsert(detected, active);
        setJustInserted(true);
        if (flashRef.current)
            clearTimeout(flashRef.current);
        flashRef.current = setTimeout(() => setJustInserted(false), 1200);
    };
    const insertTitle = noKeyboard
        ? 'Aucun clavier MIDI détecté'
        : noTrack
            ? 'Sélectionne d’abord une piste (clic sur son nom pour l’agrandir)'
            : detected === null
                ? 'En attente de notes…'
                : canInsert
                    ? mode === 'live'
                        ? 'Clique pour insérer l’accord dans la grille (1 ronde)'
                        : 'Clique pour insérer l’accord en notes dans la piste sélectionnée'
                    : 'Accord non identifié (notes seules)';
    return (_jsxs("div", { className: "bg-gray-900 rounded-xl border border-gray-800 p-2 sm:p-3", children: [_jsxs("div", { className: "flex items-center gap-3", children: [_jsx("span", { className: "text-lg select-none", title: "Reconnaissance d'accords (clavier MIDI)", children: "\uD83C\uDFB9" }), _jsxs("div", { className: "flex-1 min-w-0", children: [_jsxs("div", { className: "text-[10px] uppercase tracking-wider text-gray-500 truncate flex items-center gap-2", children: ["Accord d\u00E9tect\u00E9", noKeyboard
                                        ? ' · clavier non détecté'
                                        : ` · ${device}`, mode === 'navig' && (_jsxs("span", { className: "text-amber-400/80 normal-case font-bold truncate", children: ["\u2192 ", targetTrackLabel ?? 'aucune piste sélectionnée'] })), justInserted && _jsx("span", { className: "text-green-400 normal-case font-bold", children: "\u2713 ins\u00E9r\u00E9" })] }), _jsx("button", { onClick: handleInsert, disabled: insertDisabled, title: insertTitle, className: `text-5xl sm:text-6xl font-bold font-mono leading-none transition-colors ${!insertDisabled
                                    ? 'text-green-300 hover:text-green-200 cursor-pointer'
                                    : 'text-gray-600 cursor-default'}`, children: detected ? detected.label : '—' }), _jsx("div", { className: `text-2xl sm:text-3xl font-mono leading-tight mt-1 ${noteNames ? 'text-cyan-300' : 'text-gray-700'}`, children: noteNames || '· · ·' })] }), mode === 'navig' && onToggleIllumination && (_jsxs("button", { onClick: onToggleIllumination, className: `shrink-0 text-[10px] px-1.5 py-1 rounded-md border transition-colors ${illuminationEnabled
                            ? 'bg-sky-900/40 border-sky-700/50 text-sky-300'
                            : 'bg-gray-800 border-gray-700 text-gray-500'}`, title: illuminationEnabled
                            ? 'Illumination de la piste jouée : ACTIVE (désactiver)'
                            : 'Illumination de la piste jouée : désactivée (activer)', children: ["\u2728 Piste ", illuminationEnabled ? 'ON' : 'OFF'] })), mode === 'live' && onGoNavig && (_jsx("button", { onClick: onGoNavig, className: "shrink-0 px-2 py-1 text-[10px] font-bold rounded-md border transition-colors bg-violet-900/40 border-violet-700/50 text-violet-300 hover:bg-violet-800/40", title: "Passer en mode Navig (vue DAW : mixeur, pistes, piano roll)", children: "\uD83D\uDCF1 Navig." })), mode === 'navig' && onGoLive && (_jsx("button", { onClick: onGoLive, className: "shrink-0 px-2 py-1 text-[10px] font-bold rounded-md border transition-colors bg-[#223a5a] text-[#8fb8e8] border-[#2f4a6e] hover:bg-[#2a4a70]", title: "Revenir au mode Live (MIDI temps r\u00E9el)", children: "\uD83D\uDDA5 Live" })), _jsxs("button", { onClick: cycleDelay, className: "shrink-0 text-[10px] px-1.5 py-1 rounded-md bg-gray-800 border border-gray-700 text-gray-400 hover:text-gray-200 transition-colors", title: `Insertion automatique après ${delayS} s d'appui prolongé (clique pour changer)`, children: ["\u23F1 ", delayS, "s"] }), canInsert && (_jsx("button", { onClick: handleInsert, disabled: noTrack, className: `shrink-0 text-xs px-2 py-1 rounded-md border transition-colors ${noTrack
                            ? 'bg-gray-800 border-gray-700 text-gray-600 cursor-not-allowed'
                            : 'bg-green-900/40 border-green-700/40 text-green-300 hover:bg-green-800/40'}`, title: noTrack ? 'Sélectionne d’abord une piste' : mode === 'live' ? "Insérer l'accord dans la grille (1 ronde)" : "Insérer l'accord en notes dans la piste sélectionnée", children: mode === 'live' ? '+ Grille' : '➕ Piste' }))] }), _jsx("div", { className: "overflow-x-auto mt-2 pt-2 border-t border-gray-800", children: _jsx(LivePiano, { activePitches: pianoPitches, onPlayNote: onPlayNote }) })] }));
}
export default memo(PianoLivePanel);
