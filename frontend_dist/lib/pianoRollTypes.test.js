import { describe, expect, it } from 'vitest';
import { BLACK_KEY_HEIGHT, DEFAULT_PIXELS_PER_BEAT, isBlackKey, noteName, pitchLabel, pitchToPixels, pixelsToPitch, pixelsToTime, snapToGrid, SNAP_UNITS, timeToPixels, WHITE_KEY_HEIGHT, } from './pianoRollTypes';
describe('snapToGrid — snap partagé notes ET locators (grille PianoRoll)', () => {
    it('arrondit au plus proche multiple de l’unité', () => {
        expect(snapToGrid(0.12, 1 / 16)).toBeCloseTo(0.125, 10); // 0.12 → 2/16
        expect(snapToGrid(0.1, 1 / 16)).toBeCloseTo(0.125, 10); // 0.1 → 2/16 (plus proche que 1/16)
        expect(snapToGrid(0.4, 1 / 4)).toBeCloseTo(0.5, 10); // 0.4 → 2/4
        expect(snapToGrid(0.49, 1 / 2)).toBeCloseTo(0.5, 10);
        expect(snapToGrid(0.51, 1 / 2)).toBeCloseTo(0.5, 10);
        expect(snapToGrid(0.75, 1 / 2)).toBeCloseTo(1, 10);
    });
    it('les valeurs déjà sur la grille restent inchangées (dont les entiers)', () => {
        for (const unit of SNAP_UNITS) {
            expect(snapToGrid(0, unit)).toBe(0);
            expect(snapToGrid(1, unit)).toBe(1);
            expect(snapToGrid(8, unit)).toBe(8);
        }
    });
    it('couvre les subdivisions binaires ET ternaires (triolets, sextolets)', () => {
        // Triolet de croche = 1/3, sextolet = 1/6, triolet de noire = 1/12…
        expect(snapToGrid(0.33, 1 / 3)).toBeCloseTo(1 / 3, 10);
        expect(snapToGrid(0.34, 1 / 3)).toBeCloseTo(1 / 3, 10);
        expect(snapToGrid(0.17, 1 / 6)).toBeCloseTo(1 / 6, 10);
        expect(snapToGrid(0.16, 1 / 6)).toBeCloseTo(1 / 6, 10);
        expect(snapToGrid(0.08, 1 / 12)).toBeCloseTo(1 / 12, 10);
        expect(snapToGrid(0.09, 1 / 12)).toBeCloseTo(1 / 12, 10);
    });
    it('le snap par défaut est la double croche (1/16)', () => {
        expect(snapToGrid(0.5)).toBeCloseTo(0.5, 10); // 8/16 exact
        expect(snapToGrid(0.3)).toBeCloseTo(0.3125, 10); // 5/16
    });
    it('gère les valeurs négatives et hors plage sans déborder', () => {
        expect(snapToGrid(-0.1, 1 / 4)).toBeCloseTo(0, 10);
        expect(snapToGrid(-0.6, 1 / 2)).toBeCloseTo(-0.5, 10);
        expect(Number.isFinite(snapToGrid(1e9, 1 / 32))).toBe(true);
    });
    it('ne produit jamais de flottant binaire piégé (reste sur la grille exacte)', () => {
        // Un snap doit toujours retomber sur un multiple EXACT de l’unité
        for (let i = 0; i <= 64; i++) {
            const raw = i / 7; // valeurs arbitraires
            const snapped = snapToGrid(raw, 1 / 16);
            expect(Math.abs(snapped * 16 - Math.round(snapped * 16))).toBeLessThan(1e-9);
        }
    });
});
describe('conversions pixel ↔ temps (zoom du PianoRoll)', () => {
    it('timeToPixels / pixelsToTime sont inverses (ppb par défaut et custom)', () => {
        expect(timeToPixels(0, DEFAULT_PIXELS_PER_BEAT)).toBe(0);
        expect(timeToPixels(1, 96)).toBe(96);
        expect(timeToPixels(2.5, 96)).toBe(240);
        expect(pixelsToTime(96, 96)).toBe(1);
        expect(pixelsToTime(240, 96)).toBe(2.5);
        expect(pixelsToTime(timeToPixels(3.75, 60), 60)).toBeCloseTo(3.75, 10);
    });
    it('pitchToPixels / pixelsToPitch sont inverses', () => {
        // maxPitch en haut de l’écran : pitch élevé → y petit
        expect(pitchToPixels(60, 84)).toBe((84 - 60) * WHITE_KEY_HEIGHT);
        expect(pitchToPixels(84, 84)).toBe(0);
        expect(pixelsToPitch(0, 84)).toBe(84);
        expect(pixelsToPitch((84 - 60) * WHITE_KEY_HEIGHT, 84)).toBe(60);
        expect(pixelsToPitch(pitchToPixels(72, 96), 96)).toBe(72);
    });
    it('expose les hauteurs de touches (alignement clavier)', () => {
        expect(WHITE_KEY_HEIGHT).toBe(16);
        expect(BLACK_KEY_HEIGHT).toBe(10);
        expect(BLACK_KEY_HEIGHT).toBeLessThan(WHITE_KEY_HEIGHT);
    });
});
describe('utilitaires de pitch (clavier piano)', () => {
    it('pitchLabel nomme les notes avec l’octave (dièses)', () => {
        expect(pitchLabel(60)).toBe('C4');
        expect(pitchLabel(61)).toBe('C#4');
        expect(pitchLabel(54)).toBe('F#3');
        expect(pitchLabel(70)).toBe('A#4'); // touches noires en dièses
        expect(pitchLabel(0)).toBe('C-1');
        expect(pitchLabel(127)).toBe('G9');
    });
    it('noteName renvoie le nom court', () => {
        expect(noteName(60)).toBe('C');
        expect(noteName(66)).toBe('F#');
        expect(noteName(70)).toBe('A#');
    });
    it('isBlackKey détecte les touches noires (chromatique)', () => {
        expect(isBlackKey(61)).toBe(true); // C#
        expect(isBlackKey(63)).toBe(true); // D#
        expect(isBlackKey(66)).toBe(true); // F#
        expect(isBlackKey(68)).toBe(true); // G#
        expect(isBlackKey(70)).toBe(true); // A#
        expect(isBlackKey(60)).toBe(false); // C
        expect(isBlackKey(64)).toBe(false); // E
        expect(isBlackKey(67)).toBe(false); // G
    });
});
