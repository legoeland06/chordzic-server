/**
 * pianoRollEngine — machine à états pour les interactions du PianoRoll.
 *
 * Gère la création, le déplacement, le redimensionnement et la suppression
 * de notes dans un piano roll.
 *
 * États : IDLE → CREATING / DRAGGING / RESIZING → IDLE
 */
import { generateNoteId, snapToGrid, DEFAULT_PIXELS_PER_BEAT, SNAP_UNIT, WHITE_KEY_HEIGHT, pitchToPixels, pixelsToPitch, MIN_FREE_DURATION } from './pianoRollTypes';
export function createEmptyContext() {
    return {
        state: 'IDLE',
        targetId: null,
        offsetX: 0,
        offsetY: 0,
        startTime: 0,
        startPitch: 0,
        startDuration: 0,
    };
}
/**
 * Trouve la note sous le curseur (parmi `notes`), avec une tolérance.
 * Retourne l'index et la note, ou null.
 */
export function hitTest(notes, coord, pixelsPerBeat = DEFAULT_PIXELS_PER_BEAT, maxPitch = 96) {
    for (let i = notes.length - 1; i >= 0; i--) {
        const n = notes[i];
        const x = n.startTime * pixelsPerBeat;
        const w = n.duration * pixelsPerBeat;
        const y = pitchToPixels(n.pitch, maxPitch);
        const h = WHITE_KEY_HEIGHT;
        if (coord.px >= x && coord.px <= x + w && coord.py >= y && coord.py <= y + h) {
            // Zone de resize : proportionnelle à la largeur de la note (25% max),
            // bornée entre 4 px (utilisable à faible zoom) et 10 px (confortable à
            // fort zoom). Sans ce plafond proportionnel, une note étroite à petit
            // zoom serait entièrement une zone de resize → impossible à déplacer.
            const edgeThreshold = Math.max(4, Math.min(10, w * 0.25));
            if (coord.px >= x + w - edgeThreshold) {
                return { index: i, note: n, region: 'rightEdge' };
            }
            return { index: i, note: n, region: 'body' };
        }
    }
    return null;
}
// ─── Interactions ──────────────────────────────────────────────────────
/**
 * Tentative de démarrer une interaction (clic souris).
 *
 * Retourne un nouvel InteractionContext (ou inchangé si IDLE).
 * Si le clic est sur une note → DRAGGING (body) ou RESIZING (rightEdge).
 * Si le clic est sur le vide → CREATING (une nouvelle note est créée).
 */
export function startInteraction(notes, coord, pixelsPerBeat, maxPitch, snapUnit = SNAP_UNIT, snapEnabled = true) {
    const hit = hitTest(notes, coord, pixelsPerBeat, maxPitch);
    if (hit) {
        const n = hit.note;
        if (hit.region === 'rightEdge') {
            return {
                ctx: {
                    state: 'RESIZING',
                    targetId: n.id,
                    offsetX: coord.px - (n.startTime + n.duration) * pixelsPerBeat,
                    offsetY: 0,
                    startTime: n.startTime,
                    startPitch: n.pitch,
                    startDuration: n.duration,
                },
            };
        }
        else {
            return {
                ctx: {
                    state: 'DRAGGING',
                    targetId: n.id,
                    offsetX: coord.px - n.startTime * pixelsPerBeat,
                    offsetY: coord.py - pitchToPixels(n.pitch, maxPitch),
                    startTime: n.startTime,
                    startPitch: n.pitch,
                    startDuration: n.duration,
                },
            };
        }
    }
    // Clic sur le vide → créer une nouvelle note
    const rawTime = Math.max(0, coord.px / pixelsPerBeat);
    const startTime = snapEnabled ? snapToGrid(rawTime, snapUnit) : rawTime;
    const pitch = Math.max(0, Math.min(127, pixelsToPitch(coord.py, maxPitch)));
    const newNote = {
        id: generateNoteId(),
        startTime,
        pitch,
        // Durée initiale : un cran de grille en mode snap, une noire en mode libre
        // (ajustable immédiatement au drag).
        duration: snapEnabled ? snapUnit : 0.25,
        velocity: 100,
    };
    return {
        ctx: {
            state: 'CREATING',
            targetId: newNote.id,
            offsetX: 0,
            offsetY: 0,
            startTime: newNote.startTime,
            startPitch: newNote.pitch,
            startDuration: newNote.duration,
        },
        createdNote: newNote,
    };
}
/**
 * Met à jour l'interaction en cours (mouvement de souris).
 *
 * @param ctx Contexte d'interaction courant.
 * @param coord Position actuelle de la souris.
 * @param pixelsPerBeat Zoom horizontal.
 * @param minPitch Pitch minimum visible.
 * @returns Un objet avec l'éventuelle mutation à appliquer à la note.
 */
export function updateInteraction(ctx, coord, pixelsPerBeat, maxPitch, snapUnit = SNAP_UNIT, snapEnabled = true) {
    switch (ctx.state) {
        case 'IDLE':
        case 'CREATING':
            return {};
        case 'DRAGGING': {
            const rawStart = Math.max(0, (coord.px - ctx.offsetX) / pixelsPerBeat);
            const newStartTime = snapEnabled ? snapToGrid(rawStart, snapUnit) : rawStart;
            const rawPitch = pixelsToPitch(coord.py - ctx.offsetY, maxPitch);
            const newPitch = Math.max(0, Math.min(127, rawPitch));
            return {
                note: { startTime: newStartTime, pitch: newPitch },
            };
        }
        case 'RESIZING': {
            const edgeX = coord.px - ctx.offsetX;
            const rawEnd = Math.max(snapEnabled ? snapUnit : MIN_FREE_DURATION, edgeX / pixelsPerBeat);
            const newEndTime = snapEnabled ? snapToGrid(rawEnd, snapUnit) : rawEnd;
            const newDuration = Math.max(snapEnabled ? snapUnit : MIN_FREE_DURATION, newEndTime - ctx.startTime);
            return {
                note: { duration: newDuration },
            };
        }
        default:
            return {};
    }
}
/**
 * Termine l'interaction (relâchement souris).
 * Retourne le nouvel état et l'éventuelle mutation finale.
 */
export function endInteraction(ctx, coord, pixelsPerBeat, maxPitch, snapUnit = SNAP_UNIT, snapEnabled = true) {
    const update = updateInteraction(ctx, coord, pixelsPerBeat, maxPitch, snapUnit, snapEnabled);
    const newCtx = {
        state: 'IDLE',
        targetId: null,
        offsetX: 0,
        offsetY: 0,
        startTime: 0,
        startPitch: 0,
        startDuration: 0,
    };
    return { ctx: newCtx, note: update.note };
}
/**
 * Supprime une note par ID.
 */
export function deleteNote(notes, id) {
    return notes.filter(n => n.id !== id);
}
/**
 * Registre auto-couvrant : étend [minPitch, maxPitch] pour couvrir TOUTES
 * les notes (+2 de marge), sans jamais resserrer. Retourne le nouveau
 * registre, ou null si aucun ajustement n'est nécessaire.
 *
 * ⚠️ Utilisé par le PianoRoll AVANT toute manipulation manuelle des sliders
 * Reg (l'auto-fit ne doit JAMAIS ré-étendre une plage choisie par
 * l'utilisateur : ça annulait son réglage ET décalait l'insertion des notes,
 * car userMaxPitch changeait entre le clic et le dessin).
 */
export function autoFitRange(notes, minPitch, maxPitch) {
    let mn = minPitch;
    let mx = maxPitch;
    let changed = false;
    for (const n of notes) {
        if (n.pitch < mn) {
            mn = n.pitch - 2;
            changed = true;
        }
        if (n.pitch > mx) {
            mx = n.pitch + 2;
            changed = true;
        }
    }
    if (!changed)
        return null;
    // Garde-fous : écart minimal d'une octave, bornes MIDI
    mn = Math.max(0, Math.min(mn, maxPitch - 12));
    mx = Math.min(127, Math.max(mx, minPitch + 12));
    if (mn === minPitch && mx === maxPitch)
        return null;
    return { minPitch: mn, maxPitch: mx };
}
/**
 * Calcule le ZOOM + SCROLL HORIZONTAL pour CADRER une sélection de notes
 * dans le viewport : la plage temporelle de la sélection (+ marge de 60 px)
 * tient dans la largeur, et le milieu de la sélection est centré à l'écran.
 * Bornes : zoom ∈ [minZoom, maxZoom], scroll ∈ [0, maxScroll].
 * Fonction pure (testable sans DOM).
 */
export function selectionZoomParams(selection, viewportW, pixelsPerBeat, totalBeats, minZoom, maxZoom, extraWidth) {
    if (selection.length === 0)
        return { zoom: minZoom, scrollLeft: 0 };
    const t0 = Math.min(...selection.map(n => n.startTime));
    const t1 = Math.max(...selection.map(n => n.startTime + n.duration));
    const span = Math.max(0.25, t1 - t0); // une note très courte garde un minimum
    const target = (viewportW - 60) / (span * pixelsPerBeat);
    const zoom = Math.min(maxZoom, Math.max(minZoom, target));
    const midBeat = (t0 + t1) / 2;
    const ppb = pixelsPerBeat * zoom;
    const maxScroll = Math.max(0, totalBeats * ppb + extraWidth - viewportW);
    const scrollLeft = Math.min(maxScroll, Math.max(0, midBeat * ppb - viewportW / 2));
    return { zoom, scrollLeft };
}
/**
 * FIT-TO-CONTENT vertical : adapte le registre affiché au contenu RÉEL de
 * la piste (min/max des notes ± 4 demi-tons, largeur minimale de 10
 * demi-tons, bornes MIDI 0-127). Utilisé à l'ouverture d'un piano roll
 * (intégré ou modal) : sans lui, la plage par défaut (ex. 60 demi-tons)
 * débordait de la lane et les notes étaient hors champ.
 */
export function fitRangeToContent(notes, minPitch, maxPitch) {
    if (notes.length === 0)
        return null;
    let mn = Math.min(...notes.map(n => n.pitch));
    let mx = Math.max(...notes.map(n => n.pitch));
    mn = Math.max(0, mn - 4);
    mx = Math.min(127, mx + 4);
    // Largeur minimale : 10 demi-tons (contexte lisible, clavier non écrasé)
    if (mx - mn < 10) {
        const mid = (mn + mx) / 2;
        mn = Math.max(0, Math.round(mid - 5));
        mx = Math.min(127, Math.round(mid + 5));
    }
    if (mn === minPitch && mx === maxPitch)
        return null;
    return { minPitch: mn, maxPitch: mx };
}
