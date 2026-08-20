import { describe, expect, it } from 'vitest';
import { PIANO_KEYBOARD_WIDTH, beatFromX, xFromBeat } from './pianoRollCoords';
describe('pianoRollCoords — grille alignée (clavier en colonne fixe à droite)', () => {
    it('expose la largeur de la colonne clavier', () => {
        expect(PIANO_KEYBOARD_WIDTH).toBe(100);
    });
    it('convertit beat → x écran sans décalage de clavier (origine 0)', () => {
        expect(xFromBeat(0, 60, 0)).toBe(0); // beat 0 = bord gauche, aligné lanes
        expect(xFromBeat(1, 60, 0)).toBe(60);
        expect(xFromBeat(4, 60, 120)).toBe(120);
        expect(xFromBeat(2, 60, 500)).toBe(-380); // scrollé hors champ à gauche
    });
    it('convertit x écran → beat (inverse exact)', () => {
        expect(beatFromX(0, 60, 0)).toBe(0);
        expect(beatFromX(60, 60, 0)).toBe(1);
        expect(beatFromX(120, 60, 120)).toBe(4);
        expect(beatFromX(500, 60, 100)).toBe(10);
    });
});
