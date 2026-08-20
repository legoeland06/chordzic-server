/**
 * Tests de la garde de lecture (grille Live OU notes Navig).
 */
import { describe, expect, it } from 'vitest';
import { hasPlayableContent, hasSaveableContent } from './playGuard';
describe('hasPlayableContent', () => {
    it('rien à jouer uniquement si la grille ET les notes sont vides', () => {
        expect(hasPlayableContent(0, 0)).toBe(false);
    });
    it('une grille seule suffit (mode Live classique)', () => {
        expect(hasPlayableContent(4, 0)).toBe(true);
        expect(hasPlayableContent(1, 0)).toBe(true);
    });
    it('des notes seules suffisent (mode Navig sans grille)', () => {
        expect(hasPlayableContent(0, 12)).toBe(true);
        expect(hasPlayableContent(0, 1)).toBe(true);
    });
    it('les deux présentes → jouable', () => {
        expect(hasPlayableContent(4, 12)).toBe(true);
    });
    it('ne compte que la présence (pas la taille)', () => {
        expect(hasPlayableContent(0, 0)).toBe(false);
        expect(hasPlayableContent(0, 0)).toBe(false);
    });
});
describe('hasSaveableContent — sauvegarde possible (grille texte OU notes)', () => {
    it('rien à sauvegarder si l input est vide ET aucune note', () => {
        expect(hasSaveableContent('', 0)).toBe(false);
        expect(hasSaveableContent('   ', 0)).toBe(false);
    });
    it('une grille texte suffit (mode Live)', () => {
        expect(hasSaveableContent('C G Am', 0)).toBe(true);
    });
    it('des notes seules suffisent (mode Navig, input vide)', () => {
        expect(hasSaveableContent('', 12)).toBe(true);
    });
});
