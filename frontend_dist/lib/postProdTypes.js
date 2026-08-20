/**
 * postProdTypes — types du mode PostProd (édition audio non destructive).
 *
 * Un clip est une RÉGION de lecture sur un buffer : position sur la timeline
 * (`start`), début dans le buffer source (`offset`) et durée lue (`duration`).
 * Couper / déplacer / effacer ne touche jamais aux données audio.
 */
import { SNAP_UNITS, DEFAULT_SNAP_UNIT } from './pianoRollTypes';
/** Couleurs de pistes — cohérentes avec le mode Navig (DawView). */
export const PP_TRACK_COLORS = {
    0: '#60a5fa', // Lead
    2: '#fbbf24', // Bass
    3: '#c084fc', // Nappes
    9: '#f87171', // Drums
    4: '#34d399', // Accent
};
/** Palette pour les pistes AUDIO IMPORTÉES (canaux négatifs). */
export const PP_IMPORT_COLORS = [
    '#22d3ee', '#f472b6', '#a3e635', '#fb923c',
    '#818cf8', '#facc15', '#2dd4bf', '#e879f9',
];
export function trackColorForChannel(ch) {
    if (ch >= 0)
        return PP_TRACK_COLORS[ch] ?? '#26d3ff';
    return PP_IMPORT_COLORS[((-ch) - 1) % PP_IMPORT_COLORS.length];
}
let clipSeq = 0;
/** Crée un clip couvrant TOUT le buffer (état initial d'un bounce). */
export function createFullClip(channel, duration) {
    clipSeq += 1;
    return {
        id: `clip-${channel}-${Date.now()}-${clipSeq}`,
        start: 0,
        offset: 0,
        duration,
        gain: 1,
        fadeIn: 0,
        fadeOut: 0,
    };
}
/** Subdivisions de snap disponibles — MÊMES que le mode Navig (PianoRoll) :
 * fractions de TEMPS (beat), du plus fin au plus grossier. 1/12 = triolets de
 * croches, 1/6 = triolets de noires, 1/3 = triolets de blanches, 1/24/1/18 =
 * sextolets. */
export const PP_SNAP_UNITS = SNAP_UNITS;
export const PP_DEFAULT_SNAP_UNIT = DEFAULT_SNAP_UNIT;
/** Pas de snap en SECONDES : unité (fraction de beat) × durée d'un beat. */
export function snapStepFor(tempo, unit) {
    const spb = 60 / Math.max(40, tempo);
    return unit * spb;
}
/** Snap d'une position (secondes) au plus proche multiple du pas. */
export function snapValueFor(x, tempo, unit, enabled) {
    if (!enabled)
        return x;
    const step = snapStepFor(tempo, unit);
    if (step <= 0)
        return x;
    return Math.round(x / step) * step;
}
/** Crée une piste AUDIO IMPORTÉE (canal négatif, source 'import'). */
export function createImportedTrack(buffer, index, name) {
    const channel = -(index + 1);
    return {
        channel,
        label: name,
        program: 0,
        color: trackColorForChannel(channel),
        buffer,
        volume: 1,
        pan: 0,
        mute: false,
        solo: false,
        source: 'import',
        clips: [createFullClip(channel, buffer.duration)],
    };
}
