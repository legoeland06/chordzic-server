/**
 * Tests des helpers purs PostProd (couleurs, clips, snap).
 */
import { describe, expect, it } from 'vitest';
import { createFullClip, snapStepFor, snapValueFor, trackColorForChannel, } from './postProdTypes';
describe('trackColorForChannel', () => {
    it('couleurs canoniques pour les canaux par défaut', () => {
        expect(trackColorForChannel(0)).toBe('#60a5fa'); // Lead
        expect(trackColorForChannel(2)).toBe('#fbbf24'); // Bass
        expect(trackColorForChannel(9)).toBe('#f87171'); // Drums
    });
    it('canal inconnu → couleur de repli', () => {
        expect(trackColorForChannel(7)).toBe('#26d3ff');
    });
    it('canaux négatifs (imports) → palette cyclique', () => {
        expect(trackColorForChannel(-1)).toBe('#22d3ee');
        expect(trackColorForChannel(-2)).toBe('#f472b6');
        expect(trackColorForChannel(-9)).toBe(trackColorForChannel(-1)); // cycle
    });
});
describe('createFullClip', () => {
    it('couvre tout le buffer (start 0, durée donnée, gains neutres)', () => {
        const clip = createFullClip(3, 12.5);
        expect(clip.start).toBe(0);
        expect(clip.duration).toBe(12.5);
        expect(clip.gain).toBe(1);
        expect(clip.fadeIn).toBe(0);
        expect(clip.fadeOut).toBe(0);
        expect(clip.id).toContain('clip-3-');
    });
    it('génère des ids uniques', () => {
        const a = createFullClip(3, 1).id;
        const b = createFullClip(3, 1).id;
        expect(a).not.toBe(b);
    });
});
describe('snapStepFor / snapValueFor (snap en secondes)', () => {
    it('pas = unité (fraction de beat) × durée d un beat', () => {
        expect(snapStepFor(120, 1)).toBeCloseTo(0.5, 6); // noire à 120 BPM
        expect(snapStepFor(120, 0.25)).toBeCloseTo(0.125, 6); // double croche
        expect(snapStepFor(60, 1)).toBeCloseTo(1, 6); // noire à 60 BPM
    });
    it('snapValueFor arrondit au pas le plus proche', () => {
        expect(snapValueFor(0.62, 120, 1, true)).toBeCloseTo(0.5, 6);
        expect(snapValueFor(0.63, 120, 1, true)).toBeCloseTo(0.5, 6);
        expect(snapValueFor(0.63, 120, 0.5, true)).toBeCloseTo(0.75, 6);
    });
    it('snap désactivé → position inchangée', () => {
        expect(snapValueFor(0.37, 120, 1, false)).toBe(0.37);
    });
});
