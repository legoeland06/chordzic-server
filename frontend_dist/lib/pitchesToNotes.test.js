/**
 * Tests de la conversion des notes jouées → notes de piano roll.
 */
import { describe, expect, it } from 'vitest';
import { activePitchesAt, pitchesToPianoNotes } from './pitchesToNotes';
describe('pitchesToPianoNotes', () => {
    it('conserve l ordre d appui du pianiste (pas de tri, pas de dictionnaire)', () => {
        // G4, C4, E4 joués dans cet ordre → le tableau de sortie garde cet ordre
        const notes = pitchesToPianoNotes([67, 60, 64], 4, 4);
        expect(notes.map(n => n.pitch)).toEqual([67, 60, 64]);
    });
    it('garde les hauteurs réelles jouées (inversions comprises)', () => {
        // Inversion : E4 G4 C5 → conservée telle quelle
        const notes = pitchesToPianoNotes([64, 67, 72], 0, 4);
        expect(notes.map(n => n.pitch)).toEqual([64, 67, 72]);
    });
    it('pose toutes les notes à la position et durée demandées', () => {
        const notes = pitchesToPianoNotes([60, 64, 67], 8, 2);
        expect(notes.every(n => n.startTime === 8 && n.duration === 2)).toBe(true);
        expect(notes.every(n => n.velocity === 80)).toBe(true);
        expect(new Set(notes.map(n => n.id)).size).toBe(3); // ids uniques
    });
    it('vélocité personnalisable', () => {
        const notes = pitchesToPianoNotes([60], 0, 4, 100);
        expect(notes[0].velocity).toBe(100);
    });
    it('gère un accord avec basse grave (l ordre est celui du jeu)', () => {
        // Basse G2 puis C4 E4 G4
        const notes = pitchesToPianoNotes([43, 60, 64, 67], 0, 4);
        expect(notes.map(n => n.pitch)).toEqual([43, 60, 64, 67]);
    });
    it('gère un tableau vide', () => {
        expect(pitchesToPianoNotes([], 0, 4)).toEqual([]);
    });
});
describe('activePitchesAt (illumination de la piste jouée)', () => {
    const notes = [
        { id: 'a', startTime: 0, pitch: 60, duration: 4, velocity: 80 },
        { id: 'b', startTime: 2, pitch: 64, duration: 2, velocity: 80 },
        { id: 'c', startTime: 4, pitch: 67, duration: 4, velocity: 80 },
    ];
    it('note active dans [start, start+duration[', () => {
        expect(activePitchesAt(notes, 0)).toEqual([60]);
        expect(activePitchesAt(notes, 1.5)).toEqual([60]);
        expect(activePitchesAt(notes, 3)).toEqual([60, 64]); // chevauchement
        expect(activePitchesAt(notes, 4)).toEqual([67]); // b finie, c commence
    });
    it('aucune note active hors des plages', () => {
        expect(activePitchesAt(notes, -1)).toEqual([]);
        expect(activePitchesAt(notes, 8.5)).toEqual([]);
    });
    it('piste vide → rien', () => {
        expect(activePitchesAt([], 2)).toEqual([]);
    });
});
