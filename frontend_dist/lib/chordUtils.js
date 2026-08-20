/**
 * Chord utils — helpers partagés entre audioEngine.ts, browserSynth.ts,
 * ChordApp.tsx et ChordDetailModal.tsx.
 *
 * Mutualise :
 * - backendUrl() — construction de l'URL du backend
 * - chordToNoteNames() — conversion ChordData → noms MIDI
 * - notesWithOctave() — conversion ChordData → noms avec octave (piano)
 */
import { NOTE_NAMES } from '../types/chord';
/**
 * Construit l'URL du backend en utilisant l'hôte courant.
 * Supporte le mode réseau (téléphone/tablette) et localhost.
 */
export function backendUrl() {
    if (typeof window !== 'undefined') {
        return `http://${window.location.hostname}:4000`;
    }
    return 'http://localhost:4000';
}
/**
 * Convertit un ChordData en noms de notes MIDI pour l'envoi au backend.
 *
 * Conventions d'octave :
 * - Basse (fondamentale ou alternative) → octave 2
 * - Autres notes de l'accord → octave 3
 */
export function chordToNoteNames(c) {
    const rawValues = c.midiValues;
    if (rawValues.length === 0)
        return [];
    const noteLabels = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
    // Note de basse (toujours en octave 2)
    const bassName = c.bass || c.name;
    const bassOffset = {
        C: 0, 'C#': 1, Db: 1, D: 2, 'D#': 3, Eb: 3, E: 4, F: 5, 'F#': 6, Gb: 6, G: 7, 'G#': 8, Ab: 8, A: 9, 'A#': 10, Bb: 10, B: 11,
    };
    const names = [];
    names.push(`${noteLabels[(bassOffset[bassName] ?? 0) % 12]}2`);
    // Autres notes en octave 3
    const baseOctave = 3;
    for (let i = 0; i < rawValues.length; i++) {
        const v = rawValues[i];
        const midiNumber = baseOctave * 12 + v;
        const oct = Math.floor(midiNumber / 12);
        names.push(`${noteLabels[midiNumber % 12]}${oct}`);
    }
    return names;
}
/**
 * Calcule les noms de notes avec octave (pour l'affichage sur le piano).
 * Utilise MIDI 36 (C3) comme base.
 */
export function notesWithOctave(c) {
    return c.midiValues.map(v => {
        const mn = 36 + v; // base C3 = 36
        return NOTE_NAMES[mn % 12] + Math.floor(mn / 12);
    });
}
