/**
 * Tests de l'enregistrement MIDI (Rec) — conversion et décompte.
 */
import { describe, expect, it } from 'vitest';
import { countdownClicks, recEventsToNotes } from './recMidi';
describe('recEventsToNotes — événements Rec → notes de piano roll', () => {
    it('positionne les notes à partir de la tête de lecture (tempo 120)', () => {
        // tempo 120 → 2 beats/s ; startPos = 8 beats
        const notes = recEventsToNotes([
            { pitch: 60, on_ms: 0, off_ms: 500 }, // 8 + 0 = 8.0, durée 1 beat
            { pitch: 64, on_ms: 500, off_ms: 1000 }, // 8 + 1 = 9.0
        ], 8, 120);
        expect(notes).toHaveLength(2);
        expect(notes[0].startTime).toBeCloseTo(8, 6);
        expect(notes[0].duration).toBeCloseTo(1, 6);
        expect(notes[1].startTime).toBeCloseTo(9, 6);
        expect(notes[1].duration).toBeCloseTo(1, 6);
    });
    it('conserve l ordre d appui (G, C, E)', () => {
        const notes = recEventsToNotes([
            { pitch: 67, on_ms: 0, off_ms: 250 },
            { pitch: 60, on_ms: 100, off_ms: 250 },
            { pitch: 64, on_ms: 200, off_ms: 250 },
        ], 0, 120);
        expect(notes.map(n => n.pitch)).toEqual([67, 60, 64]);
    });
    it('note non relâchée → durée d un temps (60000/tempo)', () => {
        const notes = recEventsToNotes([{ pitch: 60, on_ms: 0, off_ms: null }], 0, 120);
        expect(notes[0].duration).toBeCloseTo(1, 6); // 60000/120 = 500ms = 1 beat
    });
    it('durée minimale de 50 ms (notes très courtes)', () => {
        const notes = recEventsToNotes([{ pitch: 60, on_ms: 100, off_ms: 110 }], 0, 120);
        expect(notes[0].duration).toBeCloseTo((50 / 1000) * 2, 6); // 0.1 beat
    });
    it('gère une session vide', () => {
        expect(recEventsToNotes([], 4, 120)).toEqual([]);
    });
});
describe('countdownClicks — décompte du métronome de pré-roll', () => {
    it('4 clics espacés de 60/tempo ms, le premier immédiat', () => {
        expect(countdownClicks(120, 4)).toEqual([0, 500, 1000, 1500]);
        expect(countdownClicks(60, 4)).toEqual([0, 1000, 2000, 3000]);
    });
    it('l enregistrement démarre après le dernier clic (offset total)', () => {
        const clicks = countdownClicks(120, 4);
        const total = clicks[clicks.length - 1] + 60000 / 120;
        expect(total).toBe(2000); // 4 temps à 120 BPM
    });
});
