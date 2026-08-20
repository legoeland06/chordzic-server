import { describe, expect, it } from 'vitest';
import { getCurrentToken, getSuggestions, replaceToken } from './autocomplete';
describe('autocomplete — token courant', () => {
    it('extrait le token à la position du curseur (début/milieu/fin)', () => {
        expect(getCurrentToken('Cmaj7 Gm7', 2)).toEqual({ start: 0, end: 5, token: 'Cmaj7' });
        expect(getCurrentToken('Cmaj7 Gm7', 8)).toEqual({ start: 6, end: 9, token: 'Gm7' });
        expect(getCurrentToken('Cmaj7 Gm7', 9)).toEqual({ start: 6, end: 9, token: 'Gm7' });
        expect(getCurrentToken('Cmaj7 Gm7', 0)).toEqual({ start: 0, end: 5, token: 'Cmaj7' });
    });
    it('gère les espaces multiples et le curseur sur un espace', () => {
        // Curseur sur l’espace → token vide (rien à compléter)
        expect(getCurrentToken('C  G', 2)).toEqual({ start: 2, end: 2, token: '' });
        // Curseur juste après le C → token 'C'
        expect(getCurrentToken('C  G', 1)).toEqual({ start: 0, end: 1, token: 'C' });
        // Curseur au début du token suivant
        expect(getCurrentToken('C  G', 3)).toEqual({ start: 3, end: 4, token: 'G' });
    });
    it('replaceToken remplace exactement [start, end[', () => {
        expect(replaceToken('Cmaj7 Gm7', 6, 10, 'Dm7')).toBe('Cmaj7 Dm7');
        expect(replaceToken('Cmaj7', 0, 5, 'Dm7')).toBe('Dm7');
        expect(replaceToken('A B C', 2, 3, 'X')).toBe('A X C');
    });
});
describe('autocomplete — suggestions', () => {
    it('token vide ou espace → aucune suggestion', () => {
        expect(getSuggestions('', 'Cm7')).toEqual([]);
        expect(getSuggestions(' ', 'Cm7')).toEqual([]);
    });
    it('"4:C" complète la qualité avec le préfixe de temps conservé', () => {
        const s = getSuggestions('4:C', 'Cm7');
        expect(s.length).toBeGreaterThan(0);
        expect(s.every(x => x.startsWith('4:C'))).toBe(true);
    });
    it('"Cm" propose les qualités mineures (m, m7, m9…)', () => {
        const s = getSuggestions('Cm', '');
        expect(s.length).toBeGreaterThan(0);
        // Insensible à la casse (les qualités M7/M9 s’écrivent en majuscule)
        expect(s.every(x => x.toLowerCase().startsWith('cm'))).toBe(true);
        expect(s).toContain('Cm'); // la qualité exacte est proposée aussi
        expect(s.some(x => x.length > 2)).toBe(true); // et les extensions (m7, m9…)
    });
    it('"4:" propose le dernier accord tapé', () => {
        expect(getSuggestions('4:', 'Cm7')).toEqual(['4:Cm7']);
        expect(getSuggestions('4:', '')).toEqual([]);
    });
    it('token inconnu → aucune suggestion', () => {
        expect(getSuggestions('zzz', 'Cm7')).toEqual([]);
    });
});
