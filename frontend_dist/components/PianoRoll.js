import { jsx as _jsx, jsxs as _jsxs, Fragment as _Fragment } from "react/jsx-runtime";
/**
 * PianoRoll — composant Canvas éditable pour une piste instrumentale.
 *
 * Affiche :
 * - Un clavier de piano statique sur la gauche (touches blanches/noires)
 * - Une grille temporelle avec lignes de mesure/beat
 * - Les notes (PianoNote) sous forme de rectangles colorés
 *
 * Interactions :
 * - Clic sur vide → créer une note (snap par défaut : 1 temps)
 * - Drag centre → déplacer une note
 * - Drag bord droit → redimensionner une note
 * - Double-clic → supprimer une note
 * - Clic sur note (mode édition) → joue + sélectionne, nom au curseur et en haut
 * - Undo/redo : Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y (snapshots, 100 entrées max)
 *
 * Architecture data-driven : pas d'éléments DOM pour chaque note,
 * tout est dessiné sur un canvas avec rendu optimisé.
 */
import { memo, useRef, useEffect, useLayoutEffect, useState, useCallback } from 'react';
import { createPortal } from 'react-dom';
import PlayheadLine from './PlayheadLine';
import { pianoRollShortcut } from '../lib/pianoRollShortcuts';
import { Pencil, MousePointer2, Copy, Scissors, ClipboardPaste, Trash2, Undo2, Redo2, Play, Pause, Magnet, Grid3x3, Group, Ungroup, Cable, Maximize2, Piano, Scan, } from 'lucide-react';
import { xFromBeat, } from '../lib/pianoRollCoords';
import { DEFAULT_PIXELS_PER_BEAT, SNAP_UNIT, SNAP_UNITS, DEFAULT_SNAP_UNIT, MIN_FREE_DURATION, WHITE_KEY_HEIGHT, PIANO_KEYBOARD_WIDTH, velocityColor, pitchLabel, isBlackKey, noteName, } from '../lib/pianoRollTypes';
import { createEmptyContext, startInteraction, updateInteraction, endInteraction, deleteNote, hitTest, autoFitRange, fitRangeToContent, selectionZoomParams, } from '../lib/pianoRollEngine';
import { getProjectClipboard, setProjectClipboard, subscribeProjectClipboard, } from '../lib/projectClipboard';
// ─── Constantes ─────────────────────────────────────────────────────────
/** Nombre maximal d'entrées de l'historique undo/redo. */
const MAX_HISTORY = 100;
/** Deadzone (px) : mouvement de souris sous ce seuil = clic simple, pas de drag. */
const CLICK_DEADZONE_PX = 5;
// ─── Composant ──────────────────────────────────────────────────────────
function PianoRoll({ notes, onNotesChange, trackLabel, channel, isDrum = false, accentColor, minPitch, maxPitch, pixelsPerBeat = DEFAULT_PIXELS_PER_BEAT, height = 400, embedded = false, onClose, onPlayMidi, onPreviewNote, onExpand, keysVisible = true, onToggleKeys, recState = 'off', onToggleRec, locL, locR, onGoToBeats, onPlayAudio, onToggleMidi, recordingNotes = [], tempo, engine, onSnapChange, totalBeats, }) {
    const canvasRef = useRef(null);
    /** Canvas de la colonne clavier (fixe à droite, hors du scroll). */
    const keysCanvasRef = useRef(null);
    const containerRef = useRef(null);
    const rootRef = useRef(null);
    // ── Plage de pitch par canal : chaque instrument voit son registre utile ──
    // (ex: la basse s'ouvre sur le grave, le lead sur le médium/aigu)
    const CHANNEL_RANGES = {
        0: [48, 96], // Lead    : C3 → C7
        2: [24, 72], // Bass    : C1 → C5
        3: [36, 84], // Nappes  : C2 → C6
        4: [36, 84], // Accent  : C2 → C6
        9: [35, 81], // Drums   : plage GM (kick/snare/hihat/cymbales/toms)
    };
    const [defaultMin, defaultMax] = isDrum
        ? [35, 81] // Piste percussion (canal quelconque) : plage GM
        : CHANNEL_RANGES[channel] ?? [36, 96];
    // Registre visible : réglable par l'utilisateur (sliders dans la toolbar),
    // initialisé sur la plage du canal (ou les props explicites du parent).
    const [userMinPitch, setUserMinPitch] = useState(minPitch ?? defaultMin);
    const [userMaxPitch, setUserMaxPitch] = useState(maxPitch ?? defaultMax);
    // État machine des interactions
    const ctxRef = useRef(createEmptyContext());
    // Notes temporaires pendant le drag (pour éviter de modifier le state React à chaque frame)
    const localNotesRef = useRef(notes);
    // Note en cours de création (encore non validée)
    const [creatingNote, setCreatingNote] = useState(null);
    // Zoom / scroll
    const [scrollLeft, setScrollLeft] = useState(0);
    /** Scroll horizontal « logique » (celui du dessin), mis à jour immédiatement
     * pour que les gestes de zoom enchaînés restent centrés sur le point souris. */
    const scrollLeftRef = useRef(0);
    scrollLeftRef.current = scrollLeft;
    const [zoom, setZoom] = useState(1);
    const effectivePixelsPerBeat = pixelsPerBeat * zoom;
    // Largeur visible du conteneur : le canvas reste fixé à cette largeur,
    // c'est un spacer interne qui porte la largeur réelle du contenu.
    const [viewportW, setViewportW] = useState(800);
    // Bloque le scroll NATIF de la fenêtre quand le curseur est DANS le piano
    // roll (intégré ET modal) : le onWheel React est inopérant pour
    // preventDefault (le listener racine de React est passif pour wheel) →
    // ce listener natif NON-PASSIF annule le défilement de la page, seul le
    // piano roll répond. Couvre la zone d'édition + les portails (toolbar et
    // marge clavier, hors du composant en mode embarqué).
    useEffect(() => {
        const targets = [
            rootRef.current,
            document.getElementById('pianoroll-toolbar-slot'),
            document.getElementById('pianoroll-keys-slot'),
        ];
        const handler = (e) => e.preventDefault();
        const attached = [];
        for (const t of targets) {
            if (t) {
                t.addEventListener('wheel', handler, { passive: false });
                attached.push(t);
            }
        }
        return () => {
            for (const t of attached)
                t.removeEventListener('wheel', handler);
        };
    }, [embedded]);
    // ── Sélection / presse-papiers / vélocité ────────────────────────
    const [tool, setTool] = useState('edit');
    const [selectedIds, setSelectedIds] = useState(new Set());
    const [marquee, setMarquee] = useState(null);
    const marqueeRef = useRef(null);
    const dragSelRef = useRef(null);
    /** Version du presse-papiers global (force le re-render des boutons). */
    const [clipVersion, setClipVersion] = useState(0);
    /** Confirmation demandée pour un collage « miroir » sur une piste non vide. */
    const [confirmPaste, setConfirmPaste] = useState(null);
    /** Position (en beats) où coller : dernier endroit cliqué dans le piano roll. */
    const pasteAnchorRef = useRef(null);
    const [velValue, setVelValue] = useState(100);
    /** Durée (en subdivisions de grille) affichée par le slider de la toolbar. */
    const [durSnaps, setDurSnaps] = useState(4);
    /** Note survolée (tooltip) : pitch + position écran du curseur. */
    const [hoverInfo, setHoverInfo] = useState(null);
    // ── Lecture locale de la piste (play/pause + curseur) ─────────────
    const [pianoPlaying, setPianoPlaying] = useState('idle');
    /** Position de lecture courante en beats (lue par draw). */
    const playPosRef = useRef(0);
    // ── Subdivision de la grille (snap) : 1/1 par défaut (1 temps), 1/12 pour les triolets ──
    const [snapUnit, setSnapUnit] = useState(DEFAULT_SNAP_UNIT);
    // ── Snap magnétique ON/OFF : quand OFF, les notes se placent librement ──
    const [snapEnabled, setSnapEnabled] = useState(true);
    /** Applique le snap seulement s'il est actif (sinon position libre). */
    const snapTime = useCallback((time) => snapEnabled ? snapToGrid(time, snapUnit) : time, [snapEnabled, snapUnit]);
    /** Remonte le snap courant (locators) — ne change pas le comportement
     * local, informe juste le parent (DawView) du snap en vigueur. */
    useEffect(() => {
        onSnapChange?.(snapUnit, snapEnabled);
    }, [snapUnit, snapEnabled, onSnapChange]);
    /** Durée minimale : un cran de grille en mode snap, très fine en mode libre. */
    const minDur = snapEnabled ? snapUnit : MIN_FREE_DURATION;
    // ── Historique undo/redo (snapshots des notes) ────────────────────
    const historyRef = useRef({ undo: [], redo: [] });
    const [canUndo, setCanUndo] = useState(false);
    const [canRedo, setCanRedo] = useState(false);
    /** État des notes au début du geste souris en cours (null si aucun geste). */
    const gestureBeforeRef = useRef(null);
    /** Geste slider vélocité : état avant le geste + flag d'activité. */
    const velGestureRef = useRef(null);
    const velGestureActiveRef = useRef(false);
    /** Geste slider durée : état avant le geste + flag d'activité. */
    const durGestureRef = useRef(null);
    const durGestureActiveRef = useRef(false);
    /** Position écran du mousedown (pour distinguer clic simple vs drag). */
    const downScreenRef = useRef(null);
    /** Vrai dès que le mouvement dépasse la deadzone (drag engagé). */
    const dragEngagedRef = useRef(false);
    // ── Tactile : pointers actifs, pinch-zoom, double-tap ────────────
    const activePointersRef = useRef(new Map());
    const pinchRef = useRef({ active: false, dist0: 0, zoom0: 1, midX0: 0, scroll0: 0 });
    const lastTapRef = useRef(null);
    /** Barre de défilement horizontale dédiée (mobile). */
    const barRef = useRef(null);
    // ── Registre auto-couvrant : le registre visible s'étend pour couvrir
    // TOUTES les notes de la piste (insérées par l'utilisateur ou pré-remplies
    // automatiquement). Il ne se resserre jamais tout seul — l'utilisateur
    // garde la main avec les sliders Reg:.
    // ⚠️ Dès que l'utilisateur TOUCHE les sliders Reg (ou scrolle à la molette),
    // l'auto-fit s'efface : sinon il ré-étendait la plage dès qu'une note
    // dépassait le nouveau bord (réglage annulé) et CHANGEAIT userMaxPitch
    // entre le clic et le dessin (note insérée au mauvais endroit).
    const rangeTouchedRef = useRef(false);
    // FIT-TO-CONTENT vertical au PREMIER rendu (intégré ou modal) : la plage
    // par défaut (ex. 60 demi-tons) débordait de la lane → notes hors champ.
    const initialVerticalFitDoneRef = useRef(false);
    useEffect(() => {
        if (rangeTouchedRef.current)
            return;
        if (!initialVerticalFitDoneRef.current) {
            initialVerticalFitDoneRef.current = true;
            const fit = fitRangeToContent(notes, userMinPitch, userMaxPitch);
            if (!fit)
                return;
            if (fit.minPitch !== userMinPitch)
                setUserMinPitch(fit.minPitch);
            if (fit.maxPitch !== userMaxPitch)
                setUserMaxPitch(fit.maxPitch);
            return;
        }
        // Ensuite : extension seule (ne resserre pas la plage pendant l'édition)
        const fit = autoFitRange(notes, userMinPitch, userMaxPitch);
        if (!fit)
            return;
        if (fit.minPitch !== userMinPitch)
            setUserMinPitch(fit.minPitch);
        if (fit.maxPitch !== userMaxPitch)
            setUserMaxPitch(fit.maxPitch);
    }, [notes, userMinPitch, userMaxPitch]);
    // Recalculer la hauteur totale en fonction des touches visibles
    const totalPitchRange = userMaxPitch - userMinPitch;
    const totalHeight = totalPitchRange * WHITE_KEY_HEIGHT;
    const canvasHeight = Math.max(height, totalHeight + 40);
    // NOTE : le canvas fait la largeur du viewport (pas celle du contenu) pour
    // ne jamais dépasser la limite de taille des canvas navigateurs (~32767 px)
    // qui rendait l'affichage vide à fort zoom. Le spacer scrollable peut lui
    // être très large sans limite de layout.
    // Palette de couleurs selon le canal
    const channelColor = accentColor ?? (channel === 0 ? '#60a5fa'
        : channel === 2 ? '#fbbf24'
            : channel === 3 ? '#c084fc'
                : channel === 9 ? '#f87171'
                    : '#34d399');
    // ── Synchroniser localNotesRef ──────────────────────────────────────
    useEffect(() => {
        localNotesRef.current = notes;
    }, [notes]);
    // ── Presse-papiers global : re-render à chaque changement ────────────
    useEffect(() => {
        return subscribeProjectClipboard(() => setClipVersion(v => v + 1));
    }, []);
    // ── Dessin du canvas ────────────────────────────────────────────────
    const draw = useCallback(() => {
        const canvas = canvasRef.current;
        if (!canvas)
            return;
        const ctx = canvas.getContext('2d');
        if (!ctx)
            return;
        const dpr = window.devicePixelRatio || 1;
        const rect = canvas.getBoundingClientRect();
        canvas.width = rect.width * dpr;
        canvas.height = rect.height * dpr;
        ctx.scale(dpr, dpr);
        const w = rect.width;
        const h = rect.height;
        const ppb = effectivePixelsPerBeat;
        const currentNotes = localNotesRef.current;
        const creating = creatingNote;
        // ── Fond ──
        ctx.fillStyle = '#1a1b26';
        ctx.fillRect(0, 0, w, h);
        // ── Grille temps (lignes verticales) — origine à 0, ALIGNÉE avec les
        // lanes compactes (le clavier est une colonne FIXE à droite).
        const gridStartBeat = Math.max(0, Math.floor(scrollLeft / ppb));
        const gridEndBeat = Math.ceil((scrollLeft + w) / ppb) + 1;
        ctx.strokeStyle = '#2a2b3e';
        ctx.lineWidth = 1;
        for (let beat = gridStartBeat; beat <= gridEndBeat; beat++) {
            const x = xFromBeat(beat, ppb, scrollLeft);
            if (x < 0 || x > w)
                continue;
            const isMeasure = beat % 4 === 0;
            ctx.strokeStyle = isMeasure ? '#3a3b5e' : '#2a2b3e';
            ctx.lineWidth = isMeasure ? 1.5 : 0.5;
            ctx.beginPath();
            ctx.moveTo(x, 0);
            ctx.lineTo(x, h);
            ctx.stroke();
            // Numéros de mesure
            if (isMeasure) {
                const measure = beat / 4;
                ctx.fillStyle = '#4a4b6e';
                ctx.font = '9px monospace';
                ctx.fillText(`${measure + 1}`, x + 3, 12);
            }
        }
        // ── Subdivisions du snap (lignes fines, si l'espacement est lisible) ──
        // Masquées en mode libre : plus de grille = placement entièrement libre.
        const snapPx = snapUnit * ppb;
        if (snapEnabled && snapUnit < 1 && snapPx >= 5) {
            const steps = Math.round(1 / snapUnit);
            ctx.strokeStyle = '#22223a';
            ctx.lineWidth = 0.5;
            for (let beat = gridStartBeat; beat <= gridEndBeat; beat++) {
                for (let k = 1; k < steps; k++) {
                    const x = xFromBeat(beat + k * snapUnit, ppb, scrollLeft);
                    if (x < 0 || x > w)
                        continue;
                    ctx.beginPath();
                    ctx.moveTo(x, 0);
                    ctx.lineTo(x, h);
                    ctx.stroke();
                }
            }
        }
        // ── Rangées de notes (lignes horizontales) — pleine largeur d'édition ──
        ctx.strokeStyle = '#222233';
        ctx.lineWidth = 0.5;
        for (let pitch = userMinPitch; pitch <= userMaxPitch; pitch++) {
            const y = (userMaxPitch - pitch) * WHITE_KEY_HEIGHT + WHITE_KEY_HEIGHT - 0.5;
            if (pitch % 12 === 0) {
                ctx.strokeStyle = '#333355';
                ctx.lineWidth = 1;
            }
            else {
                ctx.strokeStyle = '#222233';
                ctx.lineWidth = 0.5;
            }
            ctx.beginPath();
            ctx.moveTo(0, y);
            ctx.lineTo(w, y);
            ctx.stroke();
        }
        // ── Dessiner les notes ──
        const drawNote = (note) => {
            const x = xFromBeat(note.startTime, ppb, scrollLeft);
            const y = (userMaxPitch - note.pitch) * WHITE_KEY_HEIGHT;
            const noteW = Math.max(3, note.duration * ppb);
            const noteH = WHITE_KEY_HEIGHT - 1;
            const isSel = selectedIds.has(note.id);
            const isGrouped = !!note.groupId;
            // Ombre
            ctx.fillStyle = 'rgba(0,0,0,0.3)';
            ctx.fillRect(x + 1, y + 1, noteW, noteH);
            if (isSel) {
                // Sélection : rouge vif + glow → bien visible d'un coup d'œil
                ctx.save();
                ctx.shadowColor = 'rgba(255,30,30,0.9)';
                ctx.shadowBlur = 9;
                ctx.fillStyle = '#ff2222';
                ctx.fillRect(x, y, noteW, noteH);
                ctx.restore();
                ctx.strokeStyle = '#ff5a5a';
                ctx.lineWidth = 2.5;
                ctx.strokeRect(x, y, noteW, noteH);
            }
            else {
                // Rectangle principal (couleur selon la vélocité)
                ctx.fillStyle = velocityColor(note.velocity);
                ctx.fillRect(x, y, noteW, noteH);
                // Bordure (plus brillante si forte vélocité)
                ctx.strokeStyle = note.velocity > 100 ? 'rgba(255,255,255,0.4)' : 'rgba(255,255,255,0.15)';
                ctx.lineWidth = 1;
                ctx.strokeRect(x, y, noteW, noteH);
            }
            // Chaîne ⛓ sur les notes groupées (si assez larges)
            if (isGrouped && noteW > 26) {
                ctx.fillStyle = 'rgba(255,255,255,0.9)';
                ctx.font = '9px monospace';
                ctx.fillText('\u26d3\ufe0f', x + noteW - 12, y + WHITE_KEY_HEIGHT - 5);
            }
            // Hauteur de note si assez large
            if (noteW > 20) {
                ctx.fillStyle = 'rgba(255,255,255,0.7)';
                ctx.font = '9px monospace';
                ctx.fillText(pitchLabel(note.pitch), x + 3, y + WHITE_KEY_HEIGHT - 4);
            }
        };
        for (const note of currentNotes) {
            drawNote(note);
        }
        if (creating) {
            drawNote(creating);
        }
        // ── Notes ENREGISTRÉES (Rec MIDI) : affichage EN DIRECT en cyan, par-
        //    dessus le contenu existant (positionnées à la tête de lecture). ──
        for (const note of recordingNotes) {
            const x = xFromBeat(note.startTime, ppb, scrollLeft);
            const y = (userMaxPitch - note.pitch) * WHITE_KEY_HEIGHT;
            const noteW = Math.max(3, note.duration * ppb);
            const noteH = WHITE_KEY_HEIGHT - 1;
            ctx.fillStyle = 'rgba(34,211,238,0.55)';
            ctx.fillRect(x, y, noteW, noteH);
            ctx.strokeStyle = '#22d3ee';
            ctx.lineWidth = 1.5;
            ctx.strokeRect(x, y, noteW, noteH);
        }
        // ── Rectangle de sélection (marquee) ──
        if (marquee) {
            const mx = Math.min(marquee.x0, marquee.x1);
            const my = Math.min(marquee.y0, marquee.y1);
            const mw = Math.abs(marquee.x1 - marquee.x0);
            const mh = Math.abs(marquee.y1 - marquee.y0);
            ctx.fillStyle = 'rgba(251,191,36,0.10)';
            ctx.fillRect(mx, my, mw, mh);
            ctx.strokeStyle = '#fbbf24';
            ctx.lineWidth = 1;
            ctx.strokeRect(mx, my, mw, mh);
        }
        // ── Curseur de lecture (ligne verticale + repère en haut) ──
        if (pianoPlaying !== 'idle' || playPosRef.current > 0) {
            const playX = xFromBeat(playPosRef.current, ppb, scrollLeft);
            ctx.strokeStyle = '#f87171';
            ctx.lineWidth = 2;
            ctx.beginPath();
            ctx.moveTo(playX, 0);
            ctx.lineTo(playX, h);
            ctx.stroke();
            ctx.fillStyle = '#f87171';
            ctx.beginPath();
            ctx.moveTo(playX - 5, 0);
            ctx.lineTo(playX + 5, 0);
            ctx.lineTo(playX, 8);
            ctx.closePath();
            ctx.fill();
        }
    }, [notes, creatingNote, effectivePixelsPerBeat, scrollLeft, userMinPitch, userMaxPitch, channelColor, height, selectedIds, marquee, pianoPlaying, snapUnit, snapEnabled, recordingNotes]);
    // ── Re-draw à chaque changement ──
    useEffect(() => {
        draw();
    }, [draw]);
    // ── Dessin de la colonne clavier (fixe à droite) ──
    useEffect(() => {
        const c = keysCanvasRef.current;
        if (!c)
            return;
        const ctx = c.getContext('2d');
        if (!ctx)
            return;
        const dpr = window.devicePixelRatio || 1;
        const rect = c.getBoundingClientRect();
        c.width = rect.width * dpr;
        c.height = rect.height * dpr;
        ctx.scale(dpr, dpr);
        const w = rect.width;
        ctx.fillStyle = '#1a1b26';
        ctx.fillRect(0, 0, w, rect.height);
        for (let pitch = userMaxPitch; pitch >= userMinPitch; pitch--) {
            const y = (userMaxPitch - pitch) * WHITE_KEY_HEIGHT;
            const isBlack = isBlackKey(pitch);
            ctx.fillStyle = isBlack ? '#2d2d3f' : '#3a3a4e';
            ctx.fillRect(0, y, w - 1, WHITE_KEY_HEIGHT);
            ctx.strokeStyle = '#4a4a5e';
            ctx.lineWidth = 0.5;
            ctx.strokeRect(0, y, w - 1, WHITE_KEY_HEIGHT);
            // Étiquette pour les Do
            if (pitch % 12 === 0) {
                ctx.fillStyle = '#8a8aae';
                ctx.font = '8px monospace';
                const label = noteName(pitch) + (Math.floor(pitch / 12) - 1);
                ctx.fillText(label, 4, y + WHITE_KEY_HEIGHT - 3);
            }
        }
    }, [userMinPitch, userMaxPitch, canvasHeight, height, keysVisible]);
    // ── Redimensionnement du canvas ──
    useEffect(() => {
        const handleResize = () => draw();
        window.addEventListener('resize', handleResize);
        return () => window.removeEventListener('resize', handleResize);
    }, [draw]);
    // ── Largeur du viewport (canvas fixe + spacer scrollable) ──
    useLayoutEffect(() => {
        const el = containerRef.current;
        if (!el)
            return;
        const update = () => setViewportW(el.clientWidth);
        update();
        const ro = new ResizeObserver(update);
        ro.observe(el);
        return () => ro.disconnect();
    }, []);
    // Redessiner quand la largeur visible change (taille du canvas)
    useEffect(() => {
        draw();
    }, [draw, viewportW]);
    // ── Gestion des événements souris ──
    /** Convertit un événement souris en coordonnées canvas. */
    const getCoord = (e) => {
        const rect = canvasRef.current.getBoundingClientRect();
        return {
            px: e.clientX - rect.left,
            py: e.clientY - rect.top,
        };
    };
    const handlePointerDown = (e) => {
        e.preventDefault();
        const canvas = canvasRef.current;
        if (canvas) {
            try {
                canvas.setPointerCapture(e.pointerId);
            }
            catch { /* déjà capturé */ }
        }
        activePointersRef.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
        // ── Pinch-zoom (2 doigts) : annule le geste d'édition en cours ──
        if (activePointersRef.current.size === 2) {
            ctxRef.current = createEmptyContext();
            setCreatingNote(null);
            marqueeRef.current = null;
            setMarquee(null);
            dragSelRef.current = null;
            gestureBeforeRef.current = null;
            downScreenRef.current = null;
            const pts = [...activePointersRef.current.values()];
            pinchRef.current = {
                active: true,
                dist0: Math.max(10, Math.hypot(pts[0].x - pts[1].x, pts[0].y - pts[1].y)),
                zoom0: zoom,
                midX0: (pts[0].x + pts[1].x) / 2,
                scroll0: scrollLeft,
            };
            return;
        }
        if (pinchRef.current.active)
            return; // 3e doigt pendant un pinch
        // ── Double-tap (mobile) : supprime la note sous le doigt ──
        const now = Date.now();
        const last = lastTapRef.current;
        const rect0 = canvas?.getBoundingClientRect();
        if (last && now - last.t < 300 && Math.hypot(e.clientX - last.x, e.clientY - last.y) < 30 && rect0) {
            lastTapRef.current = null;
            const cpx = e.clientX - rect0.left;
            {
                const adj = {
                    px: cpx + scrollLeft,
                    py: e.clientY - rect0.top,
                };
                const hit = hitTest(localNotesRef.current, adj, effectivePixelsPerBeat, userMaxPitch);
                if (hit && tool !== 'select') {
                    pushHistory(localNotesRef.current);
                    const newNotes = deleteNote(localNotesRef.current, hit.note.id);
                    localNotesRef.current = newNotes;
                    setSelectedIds(prev => {
                        if (!prev.has(hit.note.id))
                            return prev;
                        const n = new Set(prev);
                        n.delete(hit.note.id);
                        return n;
                    });
                    commitNotes(newNotes);
                    draw();
                    return;
                }
            }
        }
        else {
            lastTapRef.current = { t: now, x: e.clientX, y: e.clientY };
        }
        const coord = getCoord(e);
        // Toute la surface du canvas est éditable (le clavier est une colonne
        // fixe à DROITE, hors de la zone scrollable)
        const adjustedCoord = {
            px: coord.px + scrollLeft,
            py: coord.py,
        };
        // Ancre de collage : mémorise l'endroit cliqué (début de la note si clic
        // sur une note, sinon position snappée) → Ctrl+V colle à cet endroit.
        const hit = hitTest(localNotesRef.current, adjustedCoord, effectivePixelsPerBeat, userMaxPitch);
        pasteAnchorRef.current = hit
            ? hit.note.startTime
            : Math.max(0, snapTime(adjustedCoord.px / effectivePixelsPerBeat));
        // Capture de l'état avant le geste (pour l'undo, si le geste mute)
        gestureBeforeRef.current = snapshotNotes(localNotesRef.current);
        // Position écran du clic : permet de distinguer clic simple vs drag
        downScreenRef.current = { x: e.clientX, y: e.clientY };
        dragEngagedRef.current = false;
        // ── Mode sélection ──
        if (tool === 'select') {
            if (hit) {
                const id = hit.note.id;
                let next = selectedIds;
                if (e.shiftKey) {
                    next = new Set(selectedIds);
                    if (next.has(id))
                        next.delete(id);
                    else
                        next.add(id);
                }
                else if (!selectedIds.has(id)) {
                    // Clic sur une note groupée → tout le groupe est sélectionné
                    const gid = hit.note.groupId;
                    next = gid
                        ? new Set(localNotesRef.current.filter(n => n.groupId === gid).map(n => n.id))
                        : new Set([id]);
                }
                setSelectedIds(next);
                // Préparer le déplacement de la sélection entière
                dragSelRef.current = {
                    startPx: adjustedCoord.px,
                    startPy: adjustedCoord.py,
                    orig: localNotesRef.current.filter(n => next.has(n.id)),
                };
            }
            else {
                if (!e.shiftKey)
                    setSelectedIds(new Set());
                // Début d'un rectangle de sélection (marquee)
                const rect = { x0: coord.px, y0: coord.py, x1: coord.px, y1: coord.py };
                marqueeRef.current = rect;
                setMarquee(rect);
            }
            return;
        }
        // ── Mode édition : un clic sur une note existante la sélectionne ──
        if (hit) {
            const gid = hit.note.groupId;
            if (gid) {
                // Note groupée → la sélection devient le groupe entier
                const groupNotes = localNotesRef.current.filter(n => n.groupId === gid);
                setSelectedIds(new Set(groupNotes.map(n => n.id)));
                // Drag sur le corps → déplacement de TOUT le groupe (le bord droit
                // garde le resize individuel de la note)
                if (hit.region === 'body') {
                    dragSelRef.current = {
                        startPx: adjustedCoord.px,
                        startPy: adjustedCoord.py,
                        orig: groupNotes,
                    };
                    return;
                }
            }
            else {
                setSelectedIds(new Set([hit.note.id]));
            }
        }
        else {
            setSelectedIds(new Set());
        }
        const { ctx, createdNote } = startInteraction(localNotesRef.current, adjustedCoord, effectivePixelsPerBeat, userMaxPitch, snapUnit, snapEnabled);
        ctxRef.current = ctx;
        if (createdNote) {
            setCreatingNote(createdNote);
            // Audition immédiate de la note en cours de création
            onPreviewNote?.(createdNote.pitch);
        }
    };
    const handlePointerMove = (e) => {
        // Mettre à jour la position du pointeur
        if (activePointersRef.current.has(e.pointerId)) {
            activePointersRef.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
        }
        // ── Pinch-zoom actif : zoom autour du point médian des 2 doigts ──
        const pinch = pinchRef.current;
        if (pinch.active && activePointersRef.current.size >= 2) {
            const pts = [...activePointersRef.current.values()];
            const dist = Math.max(10, Math.hypot(pts[0].x - pts[1].x, pts[0].y - pts[1].y));
            const midX = (pts[0].x + pts[1].x) / 2;
            const rect = canvasRef.current?.getBoundingClientRect();
            if (rect) {
                const newZoom = clampZoom(pinch.zoom0 * (dist / pinch.dist0));
                zoomRef.current = newZoom;
                const ppb0 = pixelsPerBeat * pinch.zoom0;
                const ppb1 = pixelsPerBeat * newZoom;
                // Le beat sous le milieu des doigts reste à la même position écran
                const beatAtMid = (pinch.midX0 - rect.left + pinch.scroll0) / ppb0;
                const newScroll = Math.max(0, beatAtMid * ppb1 - (midX - rect.left));
                setZoom(newZoom);
                scrollLeftRef.current = newScroll;
                setScrollLeft(newScroll);
                if (containerRef.current)
                    containerRef.current.scrollLeft = newScroll;
                if (barRef.current)
                    barRef.current.scrollLeft = newScroll;
            }
            return;
        }
        const coord = getCoord(e);
        // ── Tooltip : nom de la note sous le curseur (à côté du pointeur) ──
        const canvasRect = canvasRef.current?.getBoundingClientRect();
        const inCanvas = canvasRect
            ? (e.clientX >= canvasRect.left && e.clientX <= canvasRect.right
                && e.clientY >= canvasRect.top && e.clientY <= canvasRect.bottom)
            : true;
        const canvasEl = canvasRef.current;
        if (!inCanvas) {
            setHoverInfo(null);
            if (canvasEl)
                canvasEl.style.cursor = '';
        }
        else {
            const hAdj = {
                px: coord.px + scrollLeft,
                py: coord.py,
            };
            const h = hitTest(localNotesRef.current, hAdj, effectivePixelsPerBeat, userMaxPitch);
            setHoverInfo(h ? { pitch: h.note.pitch, x: e.clientX, y: e.clientY } : null);
            // ── Curseur contextuel : flèche bidirectionnelle horizontale (↔) sur
            // le bord droit d'une note → l'utilisateur voit qu'il peut la redimensionner.
            if (canvasEl) {
                const st = ctxRef.current.state;
                if (st === 'RESIZING')
                    canvasEl.style.cursor = 'ew-resize';
                else if (st === 'DRAGGING')
                    canvasEl.style.cursor = 'grabbing';
                else if (st === 'CREATING')
                    canvasEl.style.cursor = 'crosshair';
                else if (dragSelRef.current)
                    canvasEl.style.cursor = 'grabbing';
                else if (tool === 'edit' && h?.region === 'rightEdge')
                    canvasEl.style.cursor = 'ew-resize';
                else if (tool === 'edit' && h?.region === 'body')
                    canvasEl.style.cursor = 'grab';
                else
                    canvasEl.style.cursor = '';
            }
        }
        // ── Mode sélection, ou drag de groupe initié en mode édition :
        // marquee / déplacement de sélection ──
        if (tool === 'select' || dragSelRef.current || marqueeRef.current) {
            if (marqueeRef.current) {
                const rect = { ...marqueeRef.current, x1: coord.px, y1: coord.py };
                marqueeRef.current = rect;
                setMarquee(rect);
                const sel = notesInRect(localNotesRef.current, rect);
                setSelectedIds(new Set(sel.map(n => n.id)));
                return;
            }
            if (dragSelRef.current) {
                // Deadzone : pas de déplacement tant que le curseur n'a pas bougé
                const down = downScreenRef.current;
                if (down && !dragEngagedRef.current) {
                    if (Math.hypot(e.clientX - down.x, e.clientY - down.y) >= CLICK_DEADZONE_PX)
                        dragEngagedRef.current = true;
                    else
                        return;
                }
                const { startPx, startPy, orig } = dragSelRef.current;
                const dBeat = (coord.px + scrollLeft - startPx) / effectivePixelsPerBeat;
                // Canvas : y décroît quand le pitch monte → delta inversé
                const dPitch = -Math.round((coord.py - startPy) / WHITE_KEY_HEIGHT);
                if (dBeat !== 0 || dPitch !== 0) {
                    // Toujours repartir des notes ORIGINALES + delta total (jamais
                    // des notes déjà déplacées → évite l'accumulation géométrique)
                    const moved = new Map(orig.map(n => [n.id, {
                            ...n,
                            edited: true,
                            startTime: Math.max(0, snapTime(n.startTime + dBeat)),
                            pitch: Math.min(userMaxPitch, Math.max(userMinPitch, n.pitch + dPitch)),
                        }]));
                    localNotesRef.current = localNotesRef.current.map(n => moved.get(n.id) ?? n);
                    draw();
                }
                return;
            }
            return;
        }
        if (ctxRef.current.state === 'IDLE')
            return;
        const adjustedCoord = {
            px: coord.px + scrollLeft,
            py: coord.py,
        };
        const ctx = ctxRef.current;
        if (ctx.state === 'CREATING') {
            // Ajuster la durée de la note en création
            const endTime = Math.max(0, adjustedCoord.px / effectivePixelsPerBeat);
            const snappedEnd = Math.max(minDur, snapTime(endTime));
            const startTime = ctx.startTime;
            const duration = Math.max(minDur, snappedEnd - startTime);
            if (creatingNote) {
                setCreatingNote({
                    ...creatingNote,
                    duration,
                });
            }
            return;
        }
        // Deadzone : un clic (même avec un léger tremblement) ne doit pas
        // déclencher de déplacement/redimensionnement involontaire
        const down = downScreenRef.current;
        if (down && !dragEngagedRef.current) {
            if (Math.hypot(e.clientX - down.x, e.clientY - down.y) >= CLICK_DEADZONE_PX)
                dragEngagedRef.current = true;
            else
                return;
        }
        const result = updateInteraction(ctx, adjustedCoord, effectivePixelsPerBeat, userMaxPitch, snapUnit, snapEnabled);
        if (result.note && ctx.targetId) {
            const updated = localNotesRef.current.map(n => n.id === ctx.targetId ? { ...n, ...result.note } : n);
            localNotesRef.current = updated;
            draw();
        }
    };
    const handlePointerUp = (e) => {
        activePointersRef.current.delete(e.pointerId);
        try {
            canvasRef.current?.releasePointerCapture(e.pointerId);
        }
        catch { /* déjà relâché */ }
        // Fin du pinch : pas d'édition avec un doigt restant
        if (pinchRef.current.active) {
            if (activePointersRef.current.size < 2) {
                pinchRef.current.active = false;
                ctxRef.current = createEmptyContext();
            }
            return;
        }
        const coord = getCoord(e);
        // Le drag était-il engagé ? (clic simple sinon). Refs nettoyées ici pour
        // couvrir tous les chemins de sortie.
        const dragEngaged = dragEngagedRef.current;
        dragEngagedRef.current = false;
        downScreenRef.current = null;
        // ── Mode sélection, ou drag de groupe initié en mode édition :
        // finaliser marquee / déplacement ──
        if (tool === 'select' || dragSelRef.current || marqueeRef.current) {
            if (marqueeRef.current) {
                const rect = marqueeRef.current;
                const sel = notesInRect(localNotesRef.current, rect);
                if (e.shiftKey) {
                    setSelectedIds(prev => {
                        const next = new Set(prev);
                        for (const n of sel)
                            next.add(n.id);
                        return next;
                    });
                }
                else {
                    setSelectedIds(new Set(sel.map(n => n.id)));
                }
                marqueeRef.current = null;
                setMarquee(null);
            }
            if (dragSelRef.current) {
                dragSelRef.current = null;
                if (dragEngaged) {
                    commitGesture();
                    commitNotes(localNotesRef.current);
                }
                else {
                    // Clic simple : rien n'a bougé, rien à historiser
                    gestureBeforeRef.current = null;
                }
            }
            return;
        }
        const ctx = ctxRef.current;
        if (ctx.state === 'IDLE')
            return;
        const adjustedCoord = {
            px: coord.px + scrollLeft,
            py: coord.py,
        };
        if (ctx.state === 'CREATING') {
            // Finaliser la note créée
            if (creatingNote) {
                const endTime = Math.max(0, adjustedCoord.px / effectivePixelsPerBeat);
                const snappedEnd = Math.max(minDur, snapTime(endTime));
                const duration = Math.max(minDur, snappedEnd - creatingNote.startTime);
                const finalNote = { ...creatingNote, duration, edited: true };
                const newNotes = [...localNotesRef.current, finalNote];
                localNotesRef.current = newNotes;
                commitGesture();
                commitNotes(newNotes);
                setCreatingNote(null);
            }
        }
        else if (ctx.state === 'DRAGGING' || ctx.state === 'RESIZING') {
            const result = endInteraction(ctx, adjustedCoord, effectivePixelsPerBeat, userMaxPitch, snapUnit, snapEnabled);
            ctxRef.current = result.ctx;
            if (!dragEngaged) {
                // Clic simple : pas de drag → on ne mute PAS la note (évite le
                // décalage de pitch dû au re-render et les entrées undo parasites).
                // La note joue quand même.
                gestureBeforeRef.current = null;
                const target = ctx.targetId
                    ? localNotesRef.current.find(n => n.id === ctx.targetId)
                    : undefined;
                if (target)
                    onPreviewNote?.(target.pitch);
            }
            else if (result.note && ctx.targetId) {
                const updated = localNotesRef.current.map(n => n.id === ctx.targetId ? { ...n, ...result.note, edited: true } : n);
                localNotesRef.current = updated;
                commitGesture();
                commitNotes(updated);
                // Audition de la note après déplacement/redimensionnement
                const p = result.note?.pitch;
                if (p !== undefined)
                    onPreviewNote?.(p);
            }
            draw();
        }
        ctxRef.current = createEmptyContext();
        // Curseur : retour au défaut (le prochain pointermove le re-contextualise)
        const canvasEl = canvasRef.current;
        if (canvasEl)
            canvasEl.style.cursor = '';
    };
    /** Geste interrompu (le navigateur reprend la main) : état remis à zéro. */
    const handlePointerCancel = () => {
        activePointersRef.current.clear();
        pinchRef.current.active = false;
        ctxRef.current = createEmptyContext();
        setCreatingNote(null);
        marqueeRef.current = null;
        setMarquee(null);
        dragSelRef.current = null;
        gestureBeforeRef.current = null;
        downScreenRef.current = null;
        dragEngagedRef.current = false;
        const canvasEl = canvasRef.current;
        if (canvasEl)
            canvasEl.style.cursor = '';
    };
    const handleDoubleClick = (e) => {
        // En mode sélection, la suppression passe par Suppr/Couper
        if (tool === 'select')
            return;
        const coord = getCoord(e);
        const adjustedCoord = {
            px: coord.px + scrollLeft,
            py: coord.py,
        };
        const hit = hitTest(localNotesRef.current, adjustedCoord, effectivePixelsPerBeat, userMaxPitch);
        if (hit) {
            pushHistory(localNotesRef.current);
            const newNotes = deleteNote(localNotesRef.current, hit.note.id);
            localNotesRef.current = newNotes;
            setSelectedIds(prev => {
                if (!prev.has(hit.note.id))
                    return prev;
                const next = new Set(prev);
                next.delete(hit.note.id);
                return next;
            });
            commitNotes(newNotes);
            draw();
        }
    };
    // ── Sélection : ref synchronisée (pour les raccourcis clavier) ──
    const selectedIdsRef = useRef(new Set());
    useEffect(() => { selectedIdsRef.current = selectedIds; }, [selectedIds]);
    // ── Sliders vélocité / durée : reflètent la 1re note sélectionnée ──
    useEffect(() => {
        const first = notes.find(n => selectedIds.has(n.id));
        if (first) {
            setVelValue(first.velocity);
            setDurSnaps(Math.max(1, Math.round(first.duration / snapUnit)));
        }
    }, [selectedIds, notes, snapUnit]);
    // Clôture des gestes sliders vélocité/durée (pointer relâché n'importe où)
    useEffect(() => {
        const end = () => {
            velGestureActiveRef.current = false;
            velGestureRef.current = null;
            durGestureActiveRef.current = false;
            durGestureRef.current = null;
        };
        window.addEventListener('pointerup', end);
        window.addEventListener('pointercancel', end);
        return () => {
            window.removeEventListener('pointerup', end);
            window.removeEventListener('pointercancel', end);
        };
    }, []);
    /** Arrête la lecture locale (curseur remis à zéro). */
    const stopPlayback = useCallback(() => {
        if (pianoPlaying === 'idle' && playPosRef.current === 0)
            return;
        playPosRef.current = 0;
        setPianoPlaying('idle');
        engine?.stop();
        draw();
    }, [pianoPlaying, engine, draw]);
    /** Applique un changement de notes : la lecture s'arrête immédiatement
     * si elle est active (l'édition invalide le rendu en cours). */
    const commitNotes = useCallback((newNotes) => {
        if (pianoPlaying !== 'idle')
            stopPlayback();
        onNotesChange(channel, newNotes);
    }, [pianoPlaying, stopPlayback, onNotesChange]);
    // ── Historique undo/redo ──────────────────────────────────────────
    const snapshotNotes = useCallback((list) => list.map(n => ({ ...n })), []);
    const notesEqual = useCallback((a, b) => JSON.stringify(a) === JSON.stringify(b), []);
    /** Pousse l'état `before` (copié) dans la pile undo ; invalide le redo. */
    const pushHistory = useCallback((before) => {
        const h = historyRef.current;
        h.undo.push(snapshotNotes(before));
        if (h.undo.length > MAX_HISTORY)
            h.undo.shift();
        h.redo = [];
        setCanUndo(true);
        setCanRedo(false);
    }, [snapshotNotes]);
    /** Restaure un état de notes (utilisé par undo/redo). */
    const restoreHistory = useCallback((target) => {
        localNotesRef.current = target;
        setSelectedIds(new Set());
        setCreatingNote(null);
        ctxRef.current = createEmptyContext();
        commitNotes(target);
        draw();
    }, [commitNotes, draw]);
    const undo = useCallback(() => {
        // Pas d'undo pendant un geste en cours (drag, marquee, slider)
        if (ctxRef.current.state !== 'IDLE' || dragSelRef.current || marqueeRef.current || velGestureActiveRef.current || durGestureActiveRef.current)
            return;
        const h = historyRef.current;
        const prev = h.undo.pop();
        if (!prev)
            return;
        h.redo.push(snapshotNotes(localNotesRef.current));
        setCanUndo(h.undo.length > 0);
        setCanRedo(true);
        restoreHistory(prev);
    }, [snapshotNotes, restoreHistory]);
    const redo = useCallback(() => {
        if (ctxRef.current.state !== 'IDLE' || dragSelRef.current || marqueeRef.current || velGestureActiveRef.current || durGestureActiveRef.current)
            return;
        const h = historyRef.current;
        const next = h.redo.pop();
        if (!next)
            return;
        h.undo.push(snapshotNotes(localNotesRef.current));
        setCanRedo(h.redo.length > 0);
        setCanUndo(true);
        restoreHistory(next);
    }, [snapshotNotes, restoreHistory]);
    /** Valide le geste souris en cours : push uniquement si l'état a changé. */
    const commitGesture = useCallback(() => {
        const before = gestureBeforeRef.current;
        gestureBeforeRef.current = null;
        if (before && !notesEqual(before, localNotesRef.current)) {
            pushHistory(before);
        }
    }, [notesEqual, pushHistory]);
    // ── Helpers : sélection, presse-papiers, vélocité ────────────────
    const notesInRect = (list, rect) => {
        const left = Math.min(rect.x0, rect.x1), right = Math.max(rect.x0, rect.x1);
        const top = Math.min(rect.y0, rect.y1), bottom = Math.max(rect.y0, rect.y1);
        const ppb = effectivePixelsPerBeat;
        return list.filter(n => {
            const nx = n.startTime * ppb - scrollLeft;
            const nw = Math.max(3, n.duration * ppb);
            const ny = (userMaxPitch - n.pitch) * WHITE_KEY_HEIGHT;
            const nh = WHITE_KEY_HEIGHT - 1;
            return nx < right && nx + nw > left && ny < bottom && ny + nh > top;
        });
    };
    /** Copie dans le presse-papiers GLOBAL du projet : la sélection si
     * présente, sinon TOUTE la piste. Les notes gardent leurs positions
     * ABSOLUES (startTime/pitch/duration/velocity) → collage possible dans
     * une autre piste aux mêmes emplacements. */
    const copySelection = () => {
        const sel = localNotesRef.current.filter(n => selectedIdsRef.current.has(n.id));
        const whole = sel.length === 0;
        const src = whole ? localNotesRef.current : sel;
        if (src.length === 0)
            return;
        const minStart = Math.min(...src.map(n => n.startTime));
        setProjectClipboard({
            notes: src.map(n => ({ ...n })),
            minStart,
            sourceChannel: channel,
            sourceLabel: trackLabel,
            wholeTrack: whole,
            copiedAt: Date.now(),
        });
    };
    /** Crée les notes du collage (nouveaux IDs, groupes détachés) et les insère. */
    const buildPastedNotes = (clip, offset) => {
        const stamp = Date.now();
        // Nouveaux IDs de groupe pour la copie : les groupes internes sont
        // préservés, mais détachés des groupes d'origine (pas de fusion).
        const gidMap = new Map();
        return clip.notes.map((n, i) => {
            let gid;
            if (n.groupId) {
                if (!gidMap.has(n.groupId))
                    gidMap.set(n.groupId, `grp_${stamp}_${gidMap.size}`);
                gid = gidMap.get(n.groupId);
            }
            return {
                ...n,
                id: `pasted-${stamp}-${i}`,
                groupId: gid,
                startTime: Math.round((n.startTime + offset) * 1000) / 1000,
                edited: true,
            };
        });
    };
    /** Insère `newNotes` dans la piste (undo + sélection + commit). */
    const insertNotes = (newNotes) => {
        pushHistory(localNotesRef.current);
        const merged = [...localNotesRef.current, ...newNotes];
        localNotesRef.current = merged;
        setSelectedIds(new Set(newNotes.map(n => n.id)));
        commitNotes(merged);
        draw();
    };
    /** Collage MIROIR : même piste de destination, mêmes emplacements et
     * valeurs que la piste source. FUSIONNE avec le contenu existant (les
     * notes copiées s'ajoutent aux notes déjà présentes) ; l'état précédent
     * reste dans l'historique undo (Ctrl+Z). */
    const pasteMirror = (clip) => {
        const newNotes = buildPastedNotes(clip, 0);
        pushHistory(localNotesRef.current);
        const merged = [...localNotesRef.current, ...newNotes];
        localNotesRef.current = merged;
        setSelectedIds(new Set(newNotes.map(n => n.id)));
        commitNotes(merged);
        draw();
        setConfirmPaste(null);
    };
    /** Collage RELATIF (piste source == piste courante) : à l'endroit du
     * dernier clic, comme avant (comportement historique). */
    const pasteRelative = (clip) => {
        // Coller à l'endroit cliqué (ancre mémorisée), sinon début de la zone visible
        const base = pasteAnchorRef.current !== null
            ? pasteAnchorRef.current
            : Math.max(0, snapTime(scrollLeft / effectivePixelsPerBeat));
        // Le presse-papiers stocke des positions absolues → recalculer le décalage
        const offset = base - clip.minStart;
        const newNotes = buildPastedNotes(clip, offset);
        insertNotes(newNotes);
    };
    const pasteClipboard = () => {
        const clip = getProjectClipboard();
        if (!clip || clip.notes.length === 0)
            return;
        // Même piste → collage relatif à l'ancre (comportement historique).
        if (clip.sourceChannel === channel) {
            pasteRelative(clip);
            return;
        }
        // Autre piste → collage MIROIR aux mêmes emplacements. Si la piste
        // contient déjà des notes, demander confirmation de fusion.
        const existing = localNotesRef.current;
        if (existing.length > 0) {
            setConfirmPaste({ clip, noteCount: existing.length });
            return;
        }
        pasteMirror(clip);
    };
    /** Vide le presse-papiers global. */
    const clearClipboard = () => setProjectClipboard(null);
    const deleteSelection = () => {
        const ids = selectedIdsRef.current;
        if (ids.size === 0) {
            // Mode édition : effacer la note ciblée par le contexte
            if (ctxRef.current.targetId) {
                pushHistory(localNotesRef.current);
                const newNotes = deleteNote(localNotesRef.current, ctxRef.current.targetId);
                localNotesRef.current = newNotes;
                commitNotes(newNotes);
                ctxRef.current = createEmptyContext();
                draw();
            }
            return;
        }
        pushHistory(localNotesRef.current);
        const newNotes = localNotesRef.current.filter(n => !ids.has(n.id));
        localNotesRef.current = newNotes;
        setSelectedIds(new Set());
        commitNotes(newNotes);
        draw();
    };
    const applyVelocity = (v) => {
        const ids = selectedIdsRef.current;
        if (ids.size === 0)
            return;
        // Undo : une seule entrée par geste du slider (sinon une par tick)
        if (velGestureRef.current) {
            pushHistory(velGestureRef.current);
            velGestureRef.current = null;
        }
        else if (!velGestureActiveRef.current) {
            // Flèches clavier sur le slider : chaque pression = une entrée
            pushHistory(localNotesRef.current);
        }
        const updated = localNotesRef.current.map(n => ids.has(n.id) ? { ...n, velocity: v, edited: true } : n);
        localNotesRef.current = updated;
        setVelValue(v);
        commitNotes(updated);
        draw();
    };
    /** Applique la durée (en subdivisions de grille) à toutes les notes
     * sélectionnées. Même logique d'undo que la vélocité : une seule entrée
     * par geste du slider. */
    const applyDuration = (snaps) => {
        const ids = selectedIdsRef.current;
        if (ids.size === 0)
            return;
        const d = Math.max(snapUnit, snaps * snapUnit);
        if (durGestureRef.current) {
            pushHistory(durGestureRef.current);
            durGestureRef.current = null;
        }
        else if (!durGestureActiveRef.current) {
            pushHistory(localNotesRef.current);
        }
        const updated = localNotesRef.current.map(n => ids.has(n.id) ? { ...n, duration: d, edited: true } : n);
        localNotesRef.current = updated;
        setDurSnaps(snaps);
        commitNotes(updated);
        draw();
    };
    /** Grouper : les notes sélectionnées reçoivent le même groupId. */
    const groupSelection = () => {
        const ids = selectedIdsRef.current;
        if (ids.size < 2)
            return;
        const gid = `grp_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`;
        pushHistory(localNotesRef.current);
        const updated = localNotesRef.current.map(n => ids.has(n.id) ? { ...n, groupId: gid, edited: true } : n);
        localNotesRef.current = updated;
        commitNotes(updated);
        draw();
    };
    /** Dégrouper : retire le groupId des notes sélectionnées. */
    const ungroupSelection = () => {
        const ids = selectedIdsRef.current;
        if (ids.size === 0)
            return;
        pushHistory(localNotesRef.current);
        const updated = localNotesRef.current.map(n => ids.has(n.id) ? { ...n, groupId: undefined, edited: true } : n);
        localNotesRef.current = updated;
        commitNotes(updated);
        draw();
    };
    /** Quantisation : aligne les notes (début ET fin) sur la grille du snap
     * courant. Portée : sélection si présente, sinon toutes les notes. */
    const quantizeNotes = () => {
        const ids = selectedIdsRef.current;
        const scope = ids.size > 0
            ? new Set(ids)
            : new Set(localNotesRef.current.map(n => n.id));
        if (scope.size === 0)
            return;
        let changed = false;
        const updated = localNotesRef.current.map(n => {
            if (!scope.has(n.id))
                return n;
            const start = snapToGrid(n.startTime, snapUnit);
            const end = snapToGrid(n.startTime + n.duration, snapUnit);
            const duration = Math.max(snapUnit, end - start);
            if (start === n.startTime && duration === n.duration)
                return n;
            changed = true;
            return { ...n, startTime: start, duration, edited: true };
        });
        if (!changed)
            return;
        pushHistory(localNotesRef.current);
        localNotesRef.current = updated;
        commitNotes(updated);
        draw();
    };
    // ── Gestion du scroll horizontal ──
    const handleWheel = (e) => {
        if (e.shiftKey) {
            // Scroll horizontal avec Shift+molette
            e.preventDefault();
            const v = Math.max(0, scrollLeftRef.current + e.deltaY);
            scrollLeftRef.current = v;
            setScrollLeft(v);
        }
        else if (e.ctrlKey || e.metaKey) {
            // Zoom exponentiel nuancé, centré sur le point pointé par la souris
            e.preventDefault();
            applyZoom(zoomRef.current * Math.exp(-e.deltaY * 0.0015), e.clientX);
        }
        else {
            // Molette simple = SCROLL VERTICAL du registre : l'utilisateur atteint
            // les notes hors champ (le clavier en marge, dessiné avec le même
            // registre, suit simultanément).
            e.preventDefault();
            scrollRangeVertically(e.deltaY);
        }
    };
    /** Défile le REGISTRE vertical (molette simple) : déplace la fenêtre
     * pitch affichée d'UN DEMI-TON par cran de molette — « de case en case »
     * (feedback Eric 2026-08-20 : la quinte par cran était trop grossière).
     * Les deltas fins (trackpad) s'accumulent jusqu'à un cran entier.
     * Le scroll manuel désactive l'auto-fit (comme les sliders Reg).
     * Borné à [0, 127], largeur conservée. */
    const wheelAccRef = useRef(0);
    const scrollRangeVertically = (deltaY) => {
        rangeTouchedRef.current = true;
        const range = userMaxPitch - userMinPitch;
        wheelAccRef.current += -deltaY * 0.01; // 1 demi-ton par cran (100)
        const shift = Math.floor(wheelAccRef.current);
        if (shift !== 0)
            wheelAccRef.current -= shift;
        if (shift === 0)
            return;
        const newMin = Math.max(0, Math.min(userMinPitch + shift, 127 - range));
        const newMax = newMin + range;
        if (newMin !== userMinPitch)
            setUserMinPitch(newMin);
        if (newMax !== userMaxPitch)
            setUserMaxPitch(newMax);
    };
    const handleScroll = (e) => {
        const el = e.target;
        const pos = el.scrollLeft;
        scrollLeftRef.current = pos;
        setScrollLeft(pos);
        // Synchroniser l'autre conteneur (canvas ↔ barre de défilement)
        if (el === containerRef.current && barRef.current)
            barRef.current.scrollLeft = pos;
        else if (el === barRef.current && containerRef.current)
            containerRef.current.scrollLeft = pos;
    };
    // ── Lecture locale de la piste : play/pause + curseur ────────────
    /** Bascule lecture / pause / reprise de la piste ouverte. */
    const togglePlay = useCallback(async () => {
        if (!engine)
            return;
        if (pianoPlaying === 'playing') {
            await engine.pausePianoRoll();
            setPianoPlaying('paused');
        }
        else if (pianoPlaying === 'paused') {
            await engine.resumePianoRoll();
            setPianoPlaying('playing');
        }
        else {
            const chNotes = localNotesRef.current;
            if (chNotes.length === 0)
                return;
            playPosRef.current = 0;
            setPianoPlaying('playing');
            try {
                await engine.playPianoRollChannel(channel, chNotes, tempo);
            }
            catch (e) {
                console.error('❌ Lecture PianoRoll:', e);
                setPianoPlaying('idle');
            }
        }
    }, [engine, pianoPlaying, channel, tempo]);
    // Boucle du curseur : position audio → beats → draw + auto-scroll
    useEffect(() => {
        if (pianoPlaying !== 'playing' || !engine)
            return;
        let lastBeats = -1;
        const tick = () => {
            const dur = engine.getPianoRollDuration();
            const raw = engine.getPianoRollPositionRaw();
            // Pas encore prêt (render-wav en cours) → curseur à 0
            if (dur <= 0 || raw < 0) {
                playPosRef.current = 0;
                return;
            }
            // Position atteinte la durée du buffer → fin de lecture propre
            if (raw >= dur - 0.05) {
                stopPlayback();
                return;
            }
            const beats = (raw * tempo) / 60;
            playPosRef.current = beats;
            if (Math.abs(beats - lastBeats) > 0.0005) {
                lastBeats = beats;
                draw();
                // Auto-scroll : suivre le curseur quand il sort à droite
                const el = containerRef.current;
                if (el) {
                    const x = beats * effectivePixelsPerBeat - el.scrollLeft;
                    const margin = 80;
                    if (x > el.clientWidth - margin) {
                        el.scrollLeft += x - (el.clientWidth - margin);
                    }
                }
            }
        };
        const id = setInterval(tick, 40);
        return () => clearInterval(id);
    }, [pianoPlaying, engine, tempo, effectivePixelsPerBeat, draw, stopPlayback]);
    // Arrêt automatique de la lecture dès qu'une note est modifiée (édition)
    useEffect(() => {
        if (pianoPlaying !== 'idle')
            stopPlayback();
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [notes]);
    // ── Raccourcis clavier : sélection, copier/couper/coller, effacer ──
    useEffect(() => {
        const handleKeyDown = (e) => {
            const mod = e.ctrlKey || e.metaKey;
            const k = e.key.toLowerCase();
            if (mod && k === 'z' && e.shiftKey) {
                e.preventDefault();
                redo();
            }
            else if (mod && k === 'y') {
                e.preventDefault();
                redo();
            }
            else if (mod && k === 'z') {
                e.preventDefault();
                undo();
            }
            else if (mod && k === 'c') {
                copySelection();
            }
            else if (mod && k === 'x') {
                copySelection();
                deleteSelection();
            }
            else if (mod && k === 'v') {
                pasteClipboard();
            }
            else if (mod && k === 'a') {
                e.preventDefault();
                setSelectedIds(new Set(localNotesRef.current.map(n => n.id)));
            }
            else if (e.key === 'Delete' || e.key === 'Backspace') {
                deleteSelection();
            }
            else if (e.key === ' ') {
                // Ne pas doubler le clic quand un bouton / input a le focus
                const t = e.target;
                if (t && (t.tagName === 'BUTTON' || t.tagName === 'INPUT'))
                    return;
                e.preventDefault();
                const action = pianoRollShortcut({ key: ' ', ctrl: e.ctrlKey, meta: e.metaKey, shift: e.shiftKey });
                if (action === 'play-audio')
                    onPlayAudio?.();
                else if (action === 'play-midi')
                    onToggleMidi?.();
                else
                    togglePlay();
            }
            else if (e.key === 'Escape') {
                stopPlayback();
                onClose?.();
            }
            else {
                // Raccourcis dédiés (mapping pur, garde saisie incluse) :
                // e/v outils, Ctrl+G/U groupes, q quantiser, * REC, 0/1/2 tête de
                // lecture, o zoom sélection — puis G/H zoom horizontal (historique).
                const action = pianoRollShortcut({ key: e.key, ctrl: e.ctrlKey, meta: e.metaKey, target: e.target });
                if (action) {
                    e.preventDefault();
                    switch (action) {
                        case 'tool-edit':
                            setTool('edit');
                            break;
                        case 'tool-select':
                            setTool('select');
                            break;
                        case 'group':
                            groupSelection();
                            break;
                        case 'ungroup':
                            ungroupSelection();
                            break;
                        case 'quantize':
                            quantizeNotes();
                            break;
                        case 'rec':
                            if (onToggleRec)
                                onToggleRec(channel);
                            break;
                        case 'go-start':
                            onGoToBeats?.(0);
                            break;
                        case 'go-loc-l':
                            onGoToBeats?.(locL ?? 0);
                            break;
                        case 'go-loc-r':
                            onGoToBeats?.(locR ?? (totalBeats ?? 0));
                            break;
                        case 'zoom-selection':
                            fitZoomToSelection();
                            break;
                    }
                }
                else if (k === 'g' || k === 'h') {
                    // Zoom horizontal : G = arrière, H = avant, centré viewport
                    e.preventDefault();
                    applyZoom(zoomRef.current * (k === 'g' ? 1 / 1.25 : 1.25));
                }
            }
        };
        window.addEventListener('keydown', handleKeyDown);
        return () => window.removeEventListener('keydown', handleKeyDown);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [commitNotes, draw, onClose, scrollLeft, effectivePixelsPerBeat, undo, redo, togglePlay, stopPlayback]);
    // Presse-papiers global (lu à chaque render — clipVersion force le refresh)
    const clip = getProjectClipboard();
    void clipVersion;
    // Nombre de groupes distincts (badge du header)
    const groupCount = new Set(notes.filter(n => n.groupId).map(n => n.groupId)).size;
    // ── Barre d'outils ──
    // Durée totale : la GLOBALE des lanes si fournie (mode embarqué → même
    // échelle que les pistes compactes), sinon la fin des notes locales.
    const localTotalBeats = Math.max(16, // minimum 4 mesures
    ...notes.map(n => n.startTime + n.duration));
    const effTotalBeats = totalBeats && totalBeats > 0 ? totalBeats : localTotalBeats;
    // ── Zoom minimum DYNAMIQUE (fit-to-width) : voir TOUTE la piste d'un coup ──
    // En mode EMBARQUÉ, PAS de fit : l'échelle des lanes est la référence (le
    // fit local étirait différemment selon les notes de chaque piste → grilles
    // décalées entre pistes).
    const fitZoom = embedded ? 1 : Math.max(0.02, (viewportW - 40) / Math.max(1, effTotalBeats * pixelsPerBeat));
    const fitZoomRef = useRef(fitZoom);
    fitZoomRef.current = fitZoom;
    /** Borne le zoom entre le fit-to-width (min) et 4× (max). */
    const clampZoom = (z) => Math.min(4, Math.max(fitZoomRef.current, z));
    /** Zoom courant en ref (évite les valeurs périmées dans les handlers). */
    const zoomRef = useRef(zoom);
    zoomRef.current = zoom;
    /**
     * Applique un zoom en gardant fixe le point d'ancrage : la souris si
     * `anchorX` est fourni (molette), sinon le centre du viewport (boutons, G/H).
     */
    /** FIT-ZOOM-TO-SELECTION : recentre et zoome la vue sur les notes
     * sélectionnées — zoom temporel (plage + marge dans le viewport, borné
     * fit→4×), centrage horizontal du milieu de la sélection, et registre
     * vertical ajusté aux pitchs sélectionnés. Désactive l'auto-fit (réglage
     * manuel, comme le scroll). */
    const fitZoomToSelection = () => {
        const sel = localNotesRef.current.filter(n => selectedIds.has(n.id));
        if (sel.length === 0)
            return;
        const p = selectionZoomParams(sel, viewportW, pixelsPerBeat, effTotalBeats, fitZoomRef.current, 4, embedded ? 0 : 200);
        zoomRef.current = p.zoom;
        setZoom(p.zoom);
        scrollLeftRef.current = p.scrollLeft;
        setScrollLeft(p.scrollLeft);
        const fit = fitRangeToContent(sel, userMinPitch, userMaxPitch);
        if (fit) {
            setUserMinPitch(fit.minPitch);
            setUserMaxPitch(fit.maxPitch);
        }
        rangeTouchedRef.current = true;
    };
    const applyZoom = (target, anchorX) => {
        const el = containerRef.current;
        const rect = el?.getBoundingClientRect();
        const oldZ = zoomRef.current;
        const newZ = clampZoom(target);
        if (newZ === oldZ || !el || !rect)
            return;
        zoomRef.current = newZ;
        setZoom(newZ);
        const ppb0 = pixelsPerBeat * oldZ;
        const ppb1 = pixelsPerBeat * newZ;
        const anchor = anchorX !== undefined
            ? Math.max(0, anchorX - rect.left)
            : Math.max(0, rect.width / 2);
        // Le beat sous le point d'ancrage reste fixe à l'écran
        const beat = (anchor + scrollLeftRef.current) / ppb0;
        const maxScroll = Math.max(0, effTotalBeats * ppb1 + (embedded ? 0 : 200) - viewportW);
        const newScroll = Math.min(maxScroll, Math.max(0, beat * ppb1 - anchor));
        // Mise à jour IMMÉDIATE de la ref (les wheel events suivants du même
        // geste enchaînent sans attendre le re-render → point souris stable)
        scrollLeftRef.current = newScroll;
        setScrollLeft(newScroll);
    };
    // Re-clamp quand la durée ou le viewport changent (grille chargée, resize).
    // En MODAL, le premier rendu initialise le zoom au FIT-TO-WIDTH (voir
    // TOUTE la piste d'un coup) : sans ça, le zoom initial restait à 100 % et
    // il fallait Ctrl+molette / G-H pour dézoomer manuellement — « zoom pas
    // efficient » (feedback Eric).
    const initialFitDoneRef = useRef(false);
    useEffect(() => {
        if (!embedded && !initialFitDoneRef.current && viewportW > 100 && effTotalBeats > 0) {
            initialFitDoneRef.current = true;
            const fz = fitZoomRef.current;
            zoomRef.current = fz;
            setZoom(fz);
            return;
        }
        setZoom(z => (z < fitZoomRef.current ? fitZoomRef.current : z));
    }, [effTotalBeats, viewportW, embedded]);
    const contentWidth = effTotalBeats * effectivePixelsPerBeat + (embedded ? 0 : 200);
    /** Barre d'outils — en mode embarqué, rendue dans la zone transport de
     * DawView (portal vers #pianoroll-toolbar-slot) ; sinon rendue dans la modal. */
    const renderToolbar = () => {
        // ── Barre « studio » UNIQUE (intégré ET modal) : icônes vectorielles,
        //    groupes séparés par des filets, états actifs colorés — esprit
        //    Cubase/Pro Tools. En intégré, rendue dans la zone transport de
        //    DawView (portal) ; en modal, rendue dans la fenêtre. ──
        const btn = (active, disabled = false) => `pr-tbtn${active ? ' active' : ''}${disabled ? ' disabled' : ''}`;
        return (_jsxs("div", { className: "pianoroll-toolbar-embedded", children: [_jsxs("div", { className: "pr-group", children: [_jsx("button", { className: btn(tool === 'edit'), onClick: () => setTool('edit'), title: "Outil \u00C9dition \u2014 clic vide : cr\u00E9er \u00B7 drag : d\u00E9placer \u00B7 bord droit : redimensionner", children: _jsx(Pencil, { className: "w-3 h-3" }) }), _jsx("button", { className: btn(tool === 'select'), onClick: () => setTool('select'), title: "Outil S\u00E9lection \u2014 clic : s\u00E9lectionner \u00B7 drag vide : plage \u00B7 drag note : d\u00E9placer", children: _jsx(MousePointer2, { className: "w-3 h-3" }) })] }), _jsx("div", { className: "pr-sep" }), _jsxs("div", { className: "pr-group", children: [_jsx("button", { className: btn(false, notes.length === 0), onClick: copySelection, title: "Copier (Ctrl+C) \u2014 toute la piste si rien n'est s\u00E9lectionn\u00E9", children: _jsx(Copy, { className: "w-3 h-3" }) }), _jsx("button", { className: btn(false, selectedIds.size === 0), onClick: () => { copySelection(); deleteSelection(); }, title: "Couper (Ctrl+X)", children: _jsx(Scissors, { className: "w-3 h-3" }) }), _jsx("button", { className: btn(false, !clip), onClick: pasteClipboard, title: "Coller (Ctrl+V) \u2014 m\u00EAme piste : au clic \u00B7 autre piste : m\u00EAmes emplacements", children: _jsx(ClipboardPaste, { className: "w-3 h-3" }) }), clip && (_jsxs("span", { className: "flex items-center gap-1 px-1.5 h-6 rounded bg-yellow-900/30 border border-yellow-700/50 text-yellow-300 text-[10px] font-mono", title: `Presse-papiers : ${clip.wholeTrack ? 'toute la piste' : 'sélection'} de « ${clip.sourceLabel} » (${clip.notes.length} note(s)) — collable dans une autre piste aux mêmes emplacements`, children: ['\ud83d\udccb', " ", clip.sourceLabel, " \u00B7 ", clip.notes.length, _jsx("button", { onClick: clearClipboard, className: "ml-0.5 text-yellow-500 hover:text-white", title: "Vider le presse-papiers", children: "\u2715" })] })), _jsx("button", { className: btn(false, selectedIds.size === 0), onClick: deleteSelection, title: "Supprimer la s\u00E9lection (Suppr)", children: _jsx(Trash2, { className: "w-3 h-3" }) }), _jsx("button", { className: btn(false, selectedIds.size < 2), onClick: groupSelection, title: "Grouper la s\u00E9lection (d\u00E9place ensemble)", children: _jsx(Group, { className: "w-3 h-3" }) }), _jsx("button", { className: btn(false, selectedIds.size === 0), onClick: ungroupSelection, title: "D\u00E9grouper", children: _jsx(Ungroup, { className: "w-3 h-3" }) })] }), _jsx("div", { className: "pr-sep" }), _jsxs("div", { className: "pr-group pr-sliders", children: [_jsx("span", { className: "pr-lbl", children: "Vel" }), _jsx("input", { type: "range", min: 1, max: 127, value: velValue, onChange: (e) => applyVelocity(parseInt(e.target.value)), onPointerDown: () => { velGestureActiveRef.current = true; velGestureRef.current = snapshotNotes(localNotesRef.current); }, disabled: selectedIds.size === 0, className: "accent-amber-400", title: "V\u00E9locit\u00E9 des notes s\u00E9lectionn\u00E9es" }), _jsx("span", { className: "pr-val", children: velValue }), _jsx("span", { className: "pr-lbl", children: "Dur" }), _jsx("input", { type: "range", min: 1, max: 64, value: durSnaps, onChange: (e) => applyDuration(parseInt(e.target.value)), onPointerDown: () => { durGestureActiveRef.current = true; durGestureRef.current = snapshotNotes(localNotesRef.current); }, disabled: selectedIds.size === 0, className: "accent-sky-400 pr-dur-slider", title: "Dur\u00E9e des notes s\u00E9lectionn\u00E9es (subdivisions de grille)" }), _jsx("span", { className: "pr-val", children: durSnaps })] }), _jsx("div", { className: "pr-sep" }), _jsxs("div", { className: "pr-group", children: [_jsx("button", { className: btn(snapEnabled), onClick: () => setSnapEnabled(s => !s), title: snapEnabled ? 'Snap magnétique actif — cliquer pour libérer' : 'Snap libre — cliquer pour activer le magnétisme', children: _jsx(Magnet, { className: "w-3 h-3" }) }), _jsx("select", { value: snapUnit, onChange: (e) => setSnapUnit(parseFloat(e.target.value)), title: "Subdivision de la grille (1/32 \u2192 1/1, triolets, sextolets)", children: SNAP_UNITS.map(u => (_jsxs("option", { value: u, children: ["1/", Math.round(1 / u)] }, u))) }), _jsx("button", { className: btn(false, notes.length === 0), onClick: quantizeNotes, title: "Quantiser : aligne les notes sur la grille", children: _jsx(Grid3x3, { className: "w-3 h-3" }) })] }), _jsx("div", { className: "pr-sep" }), _jsxs("div", { className: "pr-group", children: [_jsx("button", { className: btn(pianoPlaying !== 'idle', !engine || (pianoPlaying === 'idle' && notes.length === 0)), onClick: togglePlay, title: pianoPlaying === 'playing' ? 'Pause (Espace)' : pianoPlaying === 'paused' ? 'Reprendre (Espace)' : 'Lecture de la piste (Espace)', children: pianoPlaying === 'playing' ? _jsx(Pause, { className: "w-3 h-3" }) : _jsx(Play, { className: "w-3 h-3" }) }), _jsxs("button", { className: btn(false, notes.length === 0), onClick: () => onPlayMidi?.(notes), title: "Lecture MIDI \u2014 envoie les notes sur le port MIDI choisi (instrument externe, ex. Roland)", children: [_jsx(Cable, { className: "w-3 h-3" }), _jsx("span", { className: "pr-lbl", style: { marginLeft: 2 }, children: "MIDI" })] })] }), _jsx("div", { className: "pr-sep" }), _jsxs("div", { className: "pr-group", children: [_jsx("button", { className: btn(false, !canUndo), onClick: undo, title: "Annuler (Ctrl+Z)", children: _jsx(Undo2, { className: "w-3 h-3" }) }), _jsx("button", { className: btn(false, !canRedo), onClick: redo, title: "R\u00E9tablir (Ctrl+Shift+Z / Ctrl+Y)", children: _jsx(Redo2, { className: "w-3 h-3" }) })] }), onToggleRec && (_jsxs(_Fragment, { children: [_jsx("div", { className: "pr-sep" }), _jsx("div", { className: "pr-group", children: _jsxs("button", { className: btn(recState !== 'off'), onClick: () => onToggleRec(channel), style: recState === 'on' ? { color: '#f87171' } : undefined, title: recState === 'countdown'
                                    ? 'Décompte de 4 temps en cours…'
                                    : recState === 'on'
                                        ? 'Arrêter l’enregistrement — les notes jouées sont insérées dans la piste'
                                        : 'Enregistrer ce que vous jouez sur le clavier (décompte de 4 temps, insertion à la tête de lecture)', children: [recState === 'countdown' ? '⏳' : '●', _jsx("span", { className: "pr-lbl", style: { fontSize: 9 }, children: " REC" })] }) })] })), _jsx("div", { className: "pr-sep" }), _jsx("div", { className: "pr-group", children: _jsx("button", { className: btn(false, selectedIds.size === 0), onClick: fitZoomToSelection, title: "Zoom sur la s\u00E9lection \u2014 recentre et zoome la vue sur les notes s\u00E9lectionn\u00E9es (registre vertical inclus)", children: _jsx(Scan, { className: "w-3 h-3" }) }) }), onExpand && (_jsxs(_Fragment, { children: [_jsx("div", { className: "pr-sep" }), _jsx("div", { className: "pr-group", children: _jsx("button", { className: btn(false), onClick: onExpand, title: "Ouvrir le Piano Roll en grand (modal) pour travailler \u00E0 de meilleures \u00E9chelles", children: _jsx(Maximize2, { className: "w-3 h-3" }) }) })] }))] }));
    };
    return (_jsx("div", { ref: rootRef, className: embedded ? 'w-full h-full flex flex-col bg-[#0e1016]' : 'fixed inset-0 z-50 flex items-center justify-center bg-black/70', children: _jsxs("div", { className: embedded ? 'flex flex-col flex-1 min-h-0' : 'bg-gray-900 rounded-xl border border-gray-700 shadow-2xl flex flex-col max-w-[95vw] max-h-[90vh] w-full', children: [!embedded && (_jsx(_Fragment, { children: _jsxs("div", { className: "flex items-center justify-between gap-2 flex-wrap px-3 sm:px-4 py-3 border-b border-gray-700 shrink-0", children: [_jsxs("div", { className: "flex items-center gap-2 min-w-0", children: [_jsxs("span", { className: "text-base sm:text-lg font-bold text-white truncate", children: ["\uD83C\uDFB9 ", trackLabel] }), _jsxs("span", { className: "px-2 py-0.5 rounded text-[10px] font-mono shrink-0", style: {
                                            backgroundColor: channelColor + '22',
                                            color: channelColor,
                                            border: `1px solid ${channelColor}44`,
                                        }, children: ["Canal ", channel, " \u00B7 ", notes.length, " notes"] }), groupCount > 0 && (_jsxs("span", { className: "px-2 py-0.5 rounded text-[10px] font-mono shrink-0", style: { backgroundColor: '#26d3ff22', color: '#26d3ff', border: '1px solid #26d3ff44' }, children: ['\u26d3\ufe0f', " ", groupCount, " groupe", groupCount > 1 ? 's' : ''] }))] }), _jsxs("div", { className: "flex items-center gap-1.5", children: [_jsx("button", { onClick: () => applyZoom(zoomRef.current / 1.25), className: "px-3 py-2 sm:px-2 sm:py-1 text-sm sm:text-xs bg-gray-800 text-gray-400 rounded border border-gray-700 hover:bg-gray-700 active:bg-gray-600", title: "Zoom arri\u00E8re", children: "\u2212" }), _jsxs("span", { className: "text-[10px] text-gray-500 w-8 text-center", children: [Math.round(zoom * 100), "%"] }), _jsx("button", { onClick: () => applyZoom(zoomRef.current * 1.25), className: "px-3 py-2 sm:px-2 sm:py-1 text-sm sm:text-xs bg-gray-800 text-gray-400 rounded border border-gray-700 hover:bg-gray-700 active:bg-gray-600", title: "Zoom avant", children: "+" }), !embedded && (_jsx("button", { onClick: () => { stopPlayback(); onClose?.(); }, className: "px-3 py-2 sm:px-3 sm:py-1.5 text-sm sm:text-xs bg-gray-800 text-gray-400 rounded-lg border border-gray-700 hover:text-white hover:border-gray-500 transition-colors", children: "\u2715 Fermer" }))] })] }) })), embedded ? createPortal(renderToolbar(), document.getElementById('pianoroll-toolbar-slot') ?? document.body) : renderToolbar(), _jsxs("div", { className: "flex-1 flex flex-col overflow-hidden", style: { maxHeight: embedded ? undefined : 'calc(90vh - 100px)' }, children: [_jsxs("div", { className: "flex flex-1 min-h-0", children: [_jsx("div", { ref: containerRef, className: "flex-1 min-w-0 overflow-x-auto overflow-y-hidden", "data-pr-scroll": "true", onWheel: handleWheel, onScroll: handleScroll, children: _jsxs("div", { className: "relative", style: {
                                            width: Math.max(contentWidth, viewportW),
                                            height: canvasHeight,
                                        }, children: [_jsx(PlayheadLine, { scale: effectivePixelsPerBeat, contentWidth: Math.max(contentWidth, viewportW) }), _jsx("canvas", { ref: canvasRef, className: "block cursor-crosshair sticky left-0 top-0", style: {
                                                    width: viewportW,
                                                    height: canvasHeight,
                                                    touchAction: 'none',
                                                }, onPointerDown: handlePointerDown, onPointerMove: handlePointerMove, onPointerUp: handlePointerUp, onPointerCancel: handlePointerCancel, onPointerLeave: () => {
                                                    setHoverInfo(null);
                                                    const c = canvasRef.current;
                                                    if (c)
                                                        c.style.cursor = '';
                                                }, onDoubleClick: handleDoubleClick })] }) }), !embedded && (_jsxs("div", { className: "shrink-0 relative z-10 border-l border-gray-800/60", style: { width: keysVisible ? PIANO_KEYBOARD_WIDTH : 18, height: canvasHeight }, children: [keysVisible && _jsx("canvas", { ref: keysCanvasRef, className: "block w-full h-full" }), onToggleKeys && (_jsx("button", { onClick: onToggleKeys, className: "absolute top-1 left-1/2 -translate-x-1/2 z-30 p-0.5 rounded bg-[#0d1117]/90 border border-[#1f2733] text-[#9aa3b2] hover:text-white", title: keysVisible ? 'Masquer le clavier de piano' : 'Afficher le clavier de piano', children: _jsx(Piano, { className: "w-3 h-3" }) }))] })), embedded && keysVisible && createPortal(_jsx("canvas", { ref: keysCanvasRef, className: "block w-full h-full", onWheel: handleWheel, style: { touchAction: 'none' } }), document.getElementById('pianoroll-keys-slot') ?? document.body)] }), _jsx("div", { ref: barRef, className: "shrink-0 h-10 sm:h-2.5 overflow-x-auto border-t border-gray-800 bg-gray-900", onScroll: handleScroll, children: _jsx("div", { style: { width: Math.max(contentWidth, viewportW), height: '100%' } }) })] }), confirmPaste && (_jsx("div", { className: "fixed inset-0 z-[70] flex items-center justify-center bg-black/60", children: _jsxs("div", { className: "bg-gray-900 rounded-xl border border-yellow-700/60 shadow-2xl max-w-md w-full mx-4 p-5", children: [_jsxs("div", { className: "flex items-center gap-2 mb-3", children: [_jsx("span", { className: "text-xl", children: '\ud83d\udccc' }), _jsx("h3", { className: "text-white font-bold", children: "Fusionner avec le contenu de la piste ?" })] }), _jsxs("p", { className: "text-gray-300 text-sm leading-relaxed mb-4", children: ["La piste ", _jsx("b", { className: "text-white", children: trackLabel }), " contient d\u00E9j\u00E0", ' ', _jsxs("b", { className: "text-yellow-300", children: [confirmPaste.noteCount, " note", confirmPaste.noteCount > 1 ? 's' : ''] }), ". Coller les ", _jsxs("b", { className: "text-yellow-300", children: [confirmPaste.clip.notes.length, " note(s)"] }), " de", ' ', _jsxs("b", { className: "text-white", children: ["\u00AB ", confirmPaste.clip.sourceLabel, " \u00BB"] }), " aux m\u00EAmes emplacements", ' ', _jsx("b", { className: "text-white", children: "fusionnera" }), " les deux contenus (annulable avec Ctrl+Z)."] }), _jsxs("div", { className: "flex justify-end gap-2", children: [_jsx("button", { onClick: () => setConfirmPaste(null), className: "px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 text-sm", children: "Annuler" }), _jsxs("button", { onClick: () => pasteMirror(confirmPaste.clip), className: "px-4 py-2 rounded-lg bg-yellow-700 text-white hover:bg-yellow-600 text-sm font-bold", children: ['\ud83d\udccc', " Fusionner"] })] })] }) })), hoverInfo && (_jsxs("div", { className: "fixed z-[60] pointer-events-none bg-gray-800 border border-gray-600 text-yellow-300 text-[11px] font-mono px-2 py-1 rounded shadow-lg", style: { left: hoverInfo.x + 14, top: hoverInfo.y + 16 }, children: ["\uD83C\uDFB5 ", pitchLabel(hoverInfo.pitch)] }))] }) }));
}
// ─── Helper : snapToGrid (importable localement) ────────────────────────
function snapToGrid(time, unit = SNAP_UNIT) {
    return Math.round(time / unit) * unit;
}
export default memo(PianoRoll);
