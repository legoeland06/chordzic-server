import { describe, expect, it } from 'vitest';
import { backendUrl, chordToNoteNames, notesWithOctave } from './chordUtils';
const chord = (name, quality, bass, midiValues) => ({
    time: 4, name, quality, bass, chiffrage: name + quality, notes: [], midiValues,
});
describe('chordUtils — conversion accord → notes MIDI', () => {
    it('chordToNoteNames : basse en octave 2, notes en octave 3', () => {
        // Do majeur : C en basse, puis C E G en octave 3
        const c = chord('C', '', 'C', [0, 4, 7]);
        expect(chordToNoteNames(c)).toEqual(['C2', 'C3', 'E3', 'G3']);
        // La mineur avec basse A
        const am = chord('Am', 'm', 'A', [9, 0, 4, 7]);
        expect(chordToNoteNames(am)).toEqual(['A2', 'A3', 'C3', 'E3', 'G3']);
    });
    it('chordToNoteNames : basse alternative (renversement) utilisée en basse', () => {
        // G7/B : basse B (octave 2), puis G B D F en octave 3
        const g7b = chord('G7', '7', 'B', [7, 11, 2, 5]);
        expect(chordToNoteNames(g7b)).toEqual(['B2', 'G3', 'B3', 'D3', 'F3']);
    });
    it('chordToNoteNames : gère les dièses/bémols de basse (Db, Eb, Gb, Ab, Bb)', () => {
        // Db majeur : basse Db → C#2 (même touche)
        const db = chord('Db', '', 'Db', [1, 5, 8]);
        expect(chordToNoteNames(db)[0]).toBe('C#2');
        // Bb mineur : basse Bb → A#2
        const bbm = chord('Bbm', 'm', 'Bb', [10, 1, 5, 8]);
        expect(chordToNoteNames(bbm)[0]).toBe('A#2');
    });
    it('chordToNoteNames : accord vide → liste vide', () => {
        const empty = chord('C', '', 'C', []);
        expect(chordToNoteNames(empty)).toEqual([]);
    });
});
describe('chordUtils — affichage piano', () => {
    it('notesWithOctave : base C3 (MIDI 36)', () => {
        const c = chord('C', '', 'C', [0, 4, 7]);
        expect(notesWithOctave(c)).toEqual(['C3', 'E3', 'G3']);
        const e = chord('E', '', 'E', [4, 8, 11]);
        expect(notesWithOctave(e)).toEqual(['E3', 'G#3', 'B3']);
    });
});
describe('chordUtils — backendUrl', () => {
    it('sans window (node) → localhost:4000', () => {
        expect(backendUrl()).toBe('http://localhost:4000');
    });
});
