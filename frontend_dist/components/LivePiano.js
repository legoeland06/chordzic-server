import { jsx as _jsx } from "react/jsx-runtime";
/**
 * LivePiano — piano aligné sur le clavier MIDI pour la reconnaissance
 * d'accords en mode Live.
 *
 * Portage du rendu de `rusty-chord/src/outils.rs` (app Yew) en React :
 * mêmes classes de touches (`white e`, `black cs`, …), même ordre
 * graphique, même style CSS. Les touches tenues sur le clavier MIDI
 * (Roland) s'illuminent en bleu (classe `.active`).
 *
 * Le piano est **cliquable** (onPlayNote) : un appui (souris ou doigt)
 * envoie note-on, le relâchement note-off — comme un vrai clavier. La
 * touche tenue s'illumine aussi localement. Pointer capture : la note
 * est coupée même si le curseur sort de la touche (multi-touch OK).
 *
 * Seule la partie clavier est reprise (pas le cadre bois d'origine).
 * La plage couvre par défaut l'étendue d'un clavier 88 touches
 * (A0 → C8) — alignée sur le Roland. Le piano s'adapte à la largeur du
 * conteneur (fit scale) : la font-size est recalculée à chaque
 * redimensionnement (ResizeObserver) pour tenir sur une seule ligne.
 * La logique est dans `src/lib/livePiano.ts` (testable sans DOM).
 */
import { memo, useEffect, useMemo, useRef, useState } from 'react';
import { LIVE_PIANO_MAX_PITCH, LIVE_PIANO_MIN_PITCH, activePitchSet, buildPianoKeys, computePianoFontSize, pianoWidthEm, } from '../lib/livePiano';
import './LivePiano.css';
function LivePiano({ activePitches, pitchMin = LIVE_PIANO_MIN_PITCH, pitchMax = LIVE_PIANO_MAX_PITCH, onPlayNote, }) {
    const keys = useMemo(() => buildPianoKeys(pitchMin, pitchMax), [pitchMin, pitchMax]);
    const active = useMemo(() => activePitchSet(activePitches, pitchMin, pitchMax), [activePitches, pitchMin, pitchMax]);
    const widthEm = useMemo(() => pianoWidthEm(pitchMin, pitchMax), [pitchMin, pitchMax]);
    /** Touches tenues AU CLIC (note-on envoyée, en attente du relâchement). */
    const [held, setHeld] = useState(new Set());
    // Timers de « clic court » : un clic rapide (< 300 ms) laisse sonner la
    // note ~300 ms (note-off différé) — ainsi on peut plaquer un accord en
    // cliquant vite (les notes se chevauchent), impossible à la souris avec
    // une tenue stricte (un seul pointeur). Un maintien prolongé = tenue.
    const shortClickMs = 300;
    const downAtRef = useRef(new Map());
    const shortTimersRef = useRef(new Map());
    // Échelle du piano : la font-size est recalculée pour que le piano tienne
    // dans la largeur du conteneur (null = échelle CSS par défaut, ex. SSR).
    const wrapRef = useRef(null);
    const [fontSize, setFontSize] = useState(null);
    useEffect(() => {
        const el = wrapRef.current;
        if (!el)
            return;
        const compute = () => setFontSize(computePianoFontSize(el.clientWidth, widthEm));
        compute();
        const ro = new ResizeObserver(compute);
        ro.observe(el);
        return () => ro.disconnect();
    }, [widthEm]);
    // Nettoyage de sécurité : si le composant est démonté avec des touches
    // tenues (ex. changement de mode), couper toutes les notes.
    const heldRef = useRef(held);
    heldRef.current = held;
    const onPlayNoteRef = useRef(onPlayNote);
    onPlayNoteRef.current = onPlayNote;
    useEffect(() => {
        const cb = onPlayNoteRef.current;
        if (!cb)
            return;
        for (const p of heldRef.current)
            cb(p, false);
        // Annule les note-off différés des clics courts (plus de timer orphelin).
        for (const [, t] of shortTimersRef.current)
            window.clearTimeout(t);
        shortTimersRef.current.clear();
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);
    const handleDown = (e, pitch) => {
        if (!onPlayNote)
            return;
        e.preventDefault();
        try {
            e.currentTarget.setPointerCapture(e.pointerId);
        }
        catch { /* déjà capturé */ }
        // Re-clic pendant le délai d'un clic court : annule le note-off différé
        // (sinon il couperait la nouvelle note).
        const prev = shortTimersRef.current.get(pitch);
        if (prev !== undefined) {
            window.clearTimeout(prev);
            shortTimersRef.current.delete(pitch);
        }
        downAtRef.current.set(pitch, performance.now());
        setHeld(prev => {
            if (prev.has(pitch))
                return prev;
            const n = new Set(prev);
            n.add(pitch);
            return n;
        });
        onPlayNote(pitch, true);
    };
    const handleUp = (e, pitch) => {
        if (!onPlayNote)
            return;
        if (e.currentTarget.hasPointerCapture(e.pointerId)) {
            e.currentTarget.releasePointerCapture(e.pointerId);
        }
        setHeld(prev => {
            if (!prev.has(pitch))
                return prev;
            const n = new Set(prev);
            n.delete(pitch);
            return n;
        });
        // Clic court → note-off DIFFÉRÉ (la note sonne ~300 ms au total) : un
        // accord se plaque en cliquant vite. Maintien long → coupe immédiate.
        const elapsed = performance.now() - (downAtRef.current.get(pitch) ?? performance.now());
        downAtRef.current.delete(pitch);
        if (elapsed < shortClickMs) {
            const prev = shortTimersRef.current.get(pitch);
            if (prev !== undefined)
                window.clearTimeout(prev);
            shortTimersRef.current.set(pitch, window.setTimeout(() => {
                shortTimersRef.current.delete(pitch);
                onPlayNote(pitch, false);
            }, Math.max(1, shortClickMs - elapsed)));
        }
        else {
            onPlayNote(pitch, false);
        }
    };
    return (_jsx("div", { ref: wrapRef, className: "live-piano", style: fontSize !== null ? { fontSize: `${fontSize}px` } : undefined, children: _jsx("ul", { className: "set", children: keys.map(k => (_jsx("li", { className: `${k.cls}${active.has(k.pitch) || held.has(k.pitch) ? ' active' : ''}`, title: k.noteName, onPointerDown: onPlayNote ? (e) => handleDown(e, k.pitch) : undefined, onPointerUp: onPlayNote ? (e) => handleUp(e, k.pitch) : undefined, onPointerCancel: onPlayNote ? (e) => handleUp(e, k.pitch) : undefined }, k.pitch))) }) }));
}
export default memo(LivePiano);
