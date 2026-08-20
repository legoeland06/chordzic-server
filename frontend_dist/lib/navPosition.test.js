import { describe, expect, it } from 'vitest';
import { laneRowHeight, laneTop, nameTop } from './navPosition';
import { beatsFromSeconds, computeStartBeats, estimatePositionSec, locBeatToMes, locMesToBeat, navStartAtBeats, secondsFromBeats, wrapLoopPositionSec, } from './navPosition';
describe('navPosition — position de lecture mode Navig', () => {
    it('estime la position depuis le démarrage serveur (secondes)', () => {
        expect(estimatePositionSec(1000, 1000)).toBe(0);
        expect(estimatePositionSec(1000, 2500)).toBe(1.5);
        expect(estimatePositionSec(1000, 500)).toBe(0); // jamais négatif
    });
    it('convertit beats ↔ secondes au tempo donné', () => {
        expect(secondsFromBeats(4, 120)).toBe(2);
        expect(secondsFromBeats(1, 60)).toBe(1);
        expect(secondsFromBeats(0, 120)).toBe(0);
        expect(beatsFromSeconds(2, 120)).toBe(4);
        expect(beatsFromSeconds(1, 60)).toBe(1);
        expect(beatsFromSeconds(-3, 120)).toBe(0); // jamais négatif
        expect(beatsFromSeconds(30, 60)).toBe(30);
        expect(secondsFromBeats(0, 0)).toBe(0); // tempo ≤ 0 → clampé à 1
    });
    it('calcule start_at (beats) pour le scrub séparé', () => {
        expect(navStartAtBeats(0, 120)).toBe(0);
        expect(navStartAtBeats(2, 120)).toBe(4);
        expect(navStartAtBeats(2.5, 123)).toBeCloseTo(5.125, 6);
    });
    it('wrap la position dans l\'intervalle [L, R[ (locators)', () => {
        // Pas d'intervalle → position inchangée
        expect(wrapLoopPositionSec(5, 0, 0)).toBe(5);
        expect(wrapLoopPositionSec(5, 3, 3)).toBe(5);
        // Avant L : inchangée (le 1er passage joue de start à R)
        expect(wrapLoopPositionSec(2, 4, 8)).toBe(2);
        // Dans l'intervalle : inchangée
        expect(wrapLoopPositionSec(5, 4, 8)).toBe(5);
        expect(wrapLoopPositionSec(7.999, 4, 8)).toBeCloseTo(7.999, 6);
        // À R et au-delà : wrap dans [L, R[
        expect(wrapLoopPositionSec(8, 4, 8)).toBe(4);
        expect(wrapLoopPositionSec(9.5, 4, 8)).toBeCloseTo(5.5, 6);
        expect(wrapLoopPositionSec(12, 4, 8)).toBe(4);
        expect(wrapLoopPositionSec(20, 4, 8)).toBe(4);
    });
    it('convertit beats ↔ format mesure.temps (cohérent avec la signature)', () => {
        // 4/4 : 4 temps par mesure
        expect(locBeatToMes(0, 4)).toBe('001.1');
        expect(locBeatToMes(3, 4)).toBe('001.4');
        expect(locBeatToMes(4, 4)).toBe('002.1');
        expect(locBeatToMes(18, 4)).toBe('005.3'); // 3e temps de la 5e mesure
        expect(locBeatToMes(-2, 4)).toBe('001.1'); // jamais négatif
        // 6/8 : 6 temps par mesure
        expect(locBeatToMes(26, 6)).toBe('005.3');
        // Conversion inverse
        expect(locMesToBeat('005.3', 4)).toBe(18);
        expect(locMesToBeat('001.1', 4)).toBe(0);
        expect(locMesToBeat('002.4', 4)).toBe(7);
        // Formats invalides / temps hors mesure → null
        expect(locMesToBeat('005.5', 4)).toBeNull(); // 5e temps en 4/4
        expect(locMesToBeat('abc', 4)).toBeNull();
        expect(locMesToBeat('005', 4)).toBeNull();
        expect(locMesToBeat('000.1', 4)).toBeNull(); // mesure 0
        expect(locMesToBeat('005.0', 4)).toBeNull(); // temps 0
    });
    it('computeStartBeats : le premier Play avec L ≠ 0 joue depuis le début', () => {
        // Loop activé, L = 8, tête à 0 (position par défaut) → on joue depuis 0
        // (bug corrigé : avant, on démarrait à L → début sauté / silence).
        expect(computeStartBeats(true, 8, 16, 0)).toBe(0);
        // Tête avant L (scrub à 4) → depuis la tête
        expect(computeStartBeats(true, 8, 16, 4)).toBe(4);
        // Tête dans [L, R[ → la tête
        expect(computeStartBeats(true, 8, 16, 10)).toBe(10);
        // Tête au-delà de R → retour au locator gauche
        expect(computeStartBeats(true, 8, 16, 16)).toBe(8);
        expect(computeStartBeats(true, 8, 16, 20)).toBe(8);
        // Loop désactivé → la tête, toujours
        expect(computeStartBeats(false, 8, 16, 0)).toBe(0);
        expect(computeStartBeats(false, 8, 16, 20)).toBe(20);
        // Intervalle invalide (L ≥ R) → la tête
        expect(computeStartBeats(true, 8, 8, 0)).toBe(0);
        expect(computeStartBeats(true, 0, 0, 12)).toBe(12);
        // L = 0 : comportement inchangé
        expect(computeStartBeats(true, 0, 16, 20)).toBe(0);
        expect(computeStartBeats(true, 0, 16, 5)).toBe(5);
    });
});
describe('Alignement vertical des pistes (mode Navig)', () => {
    // Valeurs réelles de DawView : LANE_COMPACT_H = 26, gap = 4, LOC_BAR_H = 20
    const COMPACT = 26, GAP = 4, HEADER = 20;
    it('une ligne de piste = lane compacte + bordure', () => {
        expect(laneRowHeight(COMPACT, GAP)).toBe(30);
    });
    it('la barre des locators décale les lanes (headerH en tête)', () => {
        expect(laneTop(0, COMPACT, GAP, HEADER)).toBe(20);
        expect(laneTop(1, COMPACT, GAP, HEADER)).toBe(50);
        expect(laneTop(2, COMPACT, GAP, HEADER)).toBe(80);
    });
    it('les NOMS de pistes restent alignés avec leurs lanes', () => {
        // Le panneau gauche est décalé de headerH : nameTop(i) + HEADER === laneTop(i)
        for (let i = 0; i < 6; i++) {
            expect(nameTop(i, COMPACT, GAP) + HEADER).toBe(laneTop(i, COMPACT, GAP, HEADER));
        }
    });
    it('le slot du clavier (piste agrandie) utilise le même top que sa lane', () => {
        const expandedIndex = 3;
        const slotTop = laneTop(expandedIndex, COMPACT, GAP, HEADER);
        // La lane agrandie est à la même position que son nom + header
        expect(slotTop).toBe(nameTop(expandedIndex, COMPACT, GAP) + HEADER);
    });
});
