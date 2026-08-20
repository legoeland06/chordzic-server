/**
 * pianoRollTypes — types purs et fonctions de conversion pour le PianoRoll.
 *
 * Définit la structure d'une note de piano (PianoNote) et les constantes
 * de rendu (pixels par beat, hauteur de touche, etc.).
 * Fournit les fonctions de conversion pixel ↔ temps/pitch avec snap to grid.
 */
// ─── Constantes de rendu ───────────────────────────────────────────────
/** Pixels par beat (zoom par défaut). */
export const DEFAULT_PIXELS_PER_BEAT = 96;
/** Snap minimum : 1/16 de beat (double croche). */
export const SNAP_UNIT = 1 / 16;
/** Subdivisions de grille disponibles (du plus fin au plus grossier).
 * Base 1/12 = multiple commun de 3 et 4 → permet triolets (1/12, 1/6, 1/3)
 * et binaires (1/16, 1/8, 1/4…). 1/18 et 1/24 couvrent les sextolets. */
export const SNAP_UNITS = [
    1 / 32, 1 / 24, 1 / 18, 1 / 16, 1 / 12, 1 / 8, 1 / 6, 1 / 4, 1 / 3, 1 / 2, 1 / 1,
];
/** Snap par défaut : 1 temps (1/1) — demandé par Eric (2026-08-20) ; les
 * subdivisions plus fines (1/16, triolets…) restent disponibles au choix. */
export const DEFAULT_SNAP_UNIT = 1 / 1;
/** Durée minimale d'une note en mode snap libre (en beats).
 * Volontairement fine (1/100 de beat ≈ 3 ticks à 288 PPQ) pour laisser
 * toute la liberté de placement, tout en restant représentable en MIDI. */
export const MIN_FREE_DURATION = 0.01;
/** Hauteur d'une touche blanche en pixels. */
export const WHITE_KEY_HEIGHT = 16;
/** Hauteur d'une touche noire en pixels (un peu moins). */
export const BLACK_KEY_HEIGHT = 10;
/** Largeur du clavier de piano (colonne gauche). */
export const PIANO_KEYBOARD_WIDTH = 100;
/** Hauteur d'une rangée de note MIDI (pitch → y). Correspond à WHITE_KEY_HEIGHT. */
export const PITCH_ROW_HEIGHT = WHITE_KEY_HEIGHT;
/** Palette de couleurs pour la vélocité (dégradé). */
export function velocityColor(velocity) {
    // De bleu foncé (faible) à jaune/rouge vif (fort)
    const t = velocity / 127;
    return `hsl(${220 - t * 160}, ${80 + t * 20}%, ${40 + t * 25}%)`;
}
/** Couleur d'une touche blanche par pitch (pour le clavier). */
export function pitchLabel(pitch) {
    const names = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
    const octave = Math.floor(pitch / 12) - 1;
    return `${names[pitch % 12]}${octave}`;
}
/** Vrai si la note MIDI est une touche noire (dièse/bémol). */
export function isBlackKey(pitch) {
    const chroma = pitch % 12;
    return [1, 3, 6, 8, 10].includes(chroma);
}
/** Nom court d'une note (ex: "C", "F#"). */
export function noteName(pitch) {
    const names = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
    return names[pitch % 12];
}
// ─── Conversions pixel ↔ temps ─────────────────────────────────────────
export function timeToPixels(time, pixelsPerBeat = DEFAULT_PIXELS_PER_BEAT) {
    return time * pixelsPerBeat;
}
export function pixelsToTime(px, pixelsPerBeat = DEFAULT_PIXELS_PER_BEAT) {
    return px / pixelsPerBeat;
}
export function pitchToPixels(pitch, maxPitch) {
    return (maxPitch - pitch) * WHITE_KEY_HEIGHT;
}
export function pixelsToPitch(px, maxPitch) {
    return maxPitch - Math.round(px / WHITE_KEY_HEIGHT);
}
/**
 * Snap une valeur temps au plus proche multiple de SNAP_UNIT.
 */
export function snapToGrid(time, unit = SNAP_UNIT) {
    return Math.round(time / unit) * unit;
}
/**
 * Snap une valeur en pixels au plus proche grid vertical (pitch).
 */
export function snapPitch(pitch) {
    return pitch; // Les pitches sont déjà discrets, pas besoin de snap
}
/**
 * Génère un ID unique pour une note.
 */
export function generateNoteId() {
    return `pn_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;
}
/**
 * Étendue des notes visibles (pitch min/max) pour un ensemble de notes donné,
 * avec une marge d'une octave. Retourne des valeurs sécurisées (0-127).
 */
export function getVisibleRange(notes, minPitch = 36, maxPitch = 96) {
    if (notes.length === 0)
        return { minPitch, maxPitch };
    const pitches = notes.map(n => n.pitch);
    const min = Math.max(0, Math.min(...pitches) - 6);
    const max = Math.min(127, Math.max(...pitches) + 6);
    // Arrondir pour commencer sur un C si possible
    const cMin = Math.floor(min / 12) * 12;
    const cMax = Math.ceil(max / 12) * 12 + 11;
    return {
        minPitch: Math.max(0, cMin),
        maxPitch: Math.min(127, cMax),
    };
}
