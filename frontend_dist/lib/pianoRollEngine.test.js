import { describe, expect, it } from 'vitest';
import { autoFitRange, fitRangeToContent, selectionZoomParams } from './pianoRollEngine';
describe('autoFitRange — registre auto-couvrant du PianoRoll', () => {
    const notes = (pitches) => pitches.map((pitch, i) => ({
        id: `n${i}`, channel: 0, startTime: i, pitch, duration: 0.25, velocity: 100,
    }));
    it('ne touche à rien si toutes les notes sont dans la plage', () => {
        expect(autoFitRange(notes([60, 64, 67]), 48, 84)).toBeNull();
        expect(autoFitRange([], 48, 84)).toBeNull();
    });
    it('étend le bord HAUT si une note dépasse (marge +2)', () => {
        const r = autoFitRange(notes([60, 86]), 48, 84);
        expect(r).toEqual({ minPitch: 48, maxPitch: 88 });
    });
    it('étend le bord BAS si une note passe sous la plage (marge −2)', () => {
        const r = autoFitRange(notes([40, 60]), 48, 84);
        expect(r).toEqual({ minPitch: 38, maxPitch: 84 });
    });
    it('ne resserre JAMAIS une plage déjà couvrante', () => {
        // Une plage large reste large même si les notes sont au centre
        expect(autoFitRange(notes([60]), 36, 96)).toBeNull();
    });
    it('respecte l’écart minimal d’une octave et les bornes MIDI', () => {
        const r = autoFitRange(notes([0]), 12, 24);
        expect(r.minPitch).toBe(0);
        expect(r.maxPitch - r.minPitch).toBeGreaterThanOrEqual(12);
        const r2 = autoFitRange(notes([127]), 100, 112);
        expect(r2.maxPitch).toBe(127);
        expect(r2.maxPitch - r2.minPitch).toBeGreaterThanOrEqual(12);
    });
});
describe('fitRangeToContent — fit vertical au contenu réel (ouverture piano roll)', () => {
    const notes = (pitches) => pitches.map((pitch, i) => ({
        id: `n${i}`, channel: 0, startTime: i, pitch, duration: 0.25, velocity: 100,
    }));
    it('réduit une plage par défaut trop large au contenu (+/−4 demi-tons)', () => {
        // Plage par défaut 36-96 (60 demi-tons) ; notes au centre → resserrée
        const r = fitRangeToContent(notes([60, 64, 67]), 36, 96);
        expect(r).toEqual({ minPitch: 56, maxPitch: 71 });
    });
    it('largeur minimale de 10 demi-tons (contexte lisible)', () => {
        const r = fitRangeToContent(notes([60]), 36, 96);
        expect(r).not.toBeNull();
        if (r) {
            expect(r.maxPitch - r.minPitch).toBe(10);
            expect(r.minPitch).toBeGreaterThanOrEqual(0);
        }
    });
    it('bornes MIDI 0-127 respectées', () => {
        const r = fitRangeToContent(notes([2]), 36, 96);
        if (r) {
            expect(r.minPitch).toBe(0); // 2-4 = -2 → clampé à 0
        }
        const r2 = fitRangeToContent(notes([126]), 36, 96);
        if (r2) {
            expect(r2.maxPitch).toBe(127);
        }
    });
    it('aucune note → aucun changement', () => {
        expect(fitRangeToContent([], 36, 96)).toBeNull();
    });
    it('plage déjà exacte → null (rien à faire)', () => {
        const r = fitRangeToContent(notes([60, 64, 67]), 56, 71);
        expect(r).toBeNull();
    });
    it('conserve l écart réel pour un contenu étendu (ex. basse + aigus)', () => {
        const r = fitRangeToContent(notes([30, 100]), 36, 96);
        expect(r).toEqual({ minPitch: 26, maxPitch: 104 });
    });
});
describe('selectionZoomParams — fit-zoom-to-selection', () => {
    const sel = (notes) => notes.map(([startTime, duration], i) => ({ id: `n${i}`, startTime, duration }));
    it('sélection vide → zoom minimum, pas de scroll', () => {
        expect(selectionZoomParams([], 800, 96, 100, 0.5, 4, 0)).toEqual({ zoom: 0.5, scrollLeft: 0 });
    });
    it('zoome pour faire tenir la sélection dans le viewport (marge 60px)', () => {
        // Sélection de 4 beats, viewport 800, ppb 96 : target = (800-60)/(4×96) ≈ 1.93
        const p = selectionZoomParams(sel([[0, 4]]), 800, 96, 100, 0.5, 4, 0);
        expect(p.zoom).toBeCloseTo(740 / 384, 2);
    });
    it('centre le milieu de la sélection dans le viewport', () => {
        // Sélection 8→12 (milieu 10), zoom 1 : scroll = 10×96 − 400 = 560
        const p = selectionZoomParams(sel([[8, 4]]), 800, 96, 100, 0.5, 4, 0);
        expect(p.zoom).toBeCloseTo(740 / 384, 2); // zoom calculé
        expect(p.scrollLeft).toBeGreaterThanOrEqual(0);
        // Vérifie le centrage : beat au centre ≈ (scroll + viewport/2) / ppb
        const ppb = 96 * p.zoom;
        const centeredBeat = (p.scrollLeft + 400) / ppb;
        expect(centeredBeat).toBeCloseTo(10, 0);
    });
    it('borne le zoom entre minZoom et maxZoom', () => {
        // Sélection énorme → zoom < min → clampé
        const big = selectionZoomParams(sel([[0, 400]]), 800, 96, 500, 0.5, 4, 0);
        expect(big.zoom).toBe(0.5);
        // Sélection minuscule → zoom > max → clampé
        const tiny = selectionZoomParams(sel([[0, 0.25]]), 800, 96, 100, 0.5, 4, 0);
        expect(tiny.zoom).toBe(4);
    });
    it('borne le scroll à [0, maxScroll]', () => {
        // Sélection à la fin → scroll plafonné
        const p = selectionZoomParams(sel([[90, 4]]), 800, 96, 100, 0.5, 4, 0);
        const maxScroll = Math.max(0, 100 * 96 * p.zoom - 800);
        expect(p.scrollLeft).toBeLessThanOrEqual(maxScroll + 0.001);
        // Sélection au début → scroll 0
        const start = selectionZoomParams(sel([[0, 4]]), 800, 96, 100, 0.5, 4, 0);
        expect(start.scrollLeft).toBe(0);
    });
    it('une note très courte garde un span minimal (0.25 beat), borné par maxZoom', () => {
        // target = 740/(0.25×96) ≈ 30.8 → clampé à 4 (le span min évite un zoom infini)
        const p = selectionZoomParams(sel([[5, 0.01]]), 800, 96, 100, 0.5, 4, 0);
        expect(p.zoom).toBe(4);
    });
});
describe('selectionZoomParams — écran pleine largeur', () => {
    const sel = (notes) => notes.map(([startTime, duration], i) => ({ id: `n${i}`, startTime, duration }));
    it('viewport très large (1920px) : la sélection est cadrée et centrée', () => {
        const p = selectionZoomParams(sel([[10, 8]]), 1920, 96, 200, 0.5, 4, 200);
        // target = (1920-60)/(8×96) ≈ 2.42 → non borné
        expect(p.zoom).toBeCloseTo(1860 / 768, 2);
        const ppb = 96 * p.zoom;
        const centeredBeat = (p.scrollLeft + 960) / ppb;
        expect(centeredBeat).toBeCloseTo(14, 0); // milieu de la sélection centré
        expect(p.scrollLeft).toBeGreaterThanOrEqual(0);
    });
    it('zoom max atteint sur très grand viewport avec une petite sélection', () => {
        const p = selectionZoomParams(sel([[0, 1]]), 1920, 96, 200, 0.5, 4, 200);
        expect(p.zoom).toBe(4);
    });
});
