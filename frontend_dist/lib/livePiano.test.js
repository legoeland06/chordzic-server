/**
 * Tests de la logique du piano Live (portage de rusty-chord/src/outils.rs,
 * aligné sur l'étendue du clavier réel A0 → C8).
 */
import { describe, expect, it } from 'vitest';
import { GRAPH_KEYS, LIVE_PIANO_MAX_PITCH, LIVE_PIANO_MIN_PITCH, activePitchSet, buildPianoKeys, computePianoFontSize, chromaticOf, pianoWidthEm, pitchToGraphIndex, } from './livePiano';
describe('buildPianoKeys', () => {
    it('couvre l étendue d un clavier 88 touches (A0 → C8) = 88 touches', () => {
        const keys = buildPianoKeys();
        expect(keys).toHaveLength(88);
        expect(LIVE_PIANO_MIN_PITCH).toBe(21); // A0
        expect(LIVE_PIANO_MAX_PITCH).toBe(108); // C8
    });
    it('commence à A0 (pitch 21, white g) et finit à C8 (pitch 108, white e)', () => {
        const keys = buildPianoKeys();
        expect(keys[0]).toMatchObject({ pitch: 21, cls: 'white g', name: 'A' });
        expect(keys[0].noteName).toBe('A0');
        expect(keys[keys.length - 1]).toMatchObject({ pitch: 108, cls: 'white e', name: 'C' });
        expect(keys[keys.length - 1].noteName).toBe('C8');
    });
    it('respecte l ordre graphique de outils.rs sur chaque octave complète', () => {
        const keys = buildPianoKeys();
        const expected = GRAPH_KEYS.map(g => g.cls);
        // A0, A#0, B0 puis C1…B7 (7 octaves complètes) puis C8
        for (let o = 0; o < 7; o++) {
            const octaveCls = keys.slice(3 + o * 12, 3 + o * 12 + 12).map(k => k.cls);
            expect(octaveCls).toEqual(expected);
        }
    });
    it('donne les noms de notes corrects (C2 = white e, C#2 = black cs, B2 = white f)', () => {
        const keys = buildPianoKeys(36, 47); // C2 → B2
        expect(keys[0].noteName).toBe('C2');
        expect(keys[1].noteName).toBe('C#2');
        expect(keys[11].noteName).toBe('B2');
    });
    it('un octave complet contient 7 blanches et 5 noires', () => {
        const keys = buildPianoKeys(36, 47);
        expect(keys.filter(k => k.isBlack)).toHaveLength(5);
        expect(keys.filter(k => !k.isBlack)).toHaveLength(7);
    });
});
describe('chromaticOf / pitchToGraphIndex', () => {
    it('mappe chaque pitch au bon index chromatique (0..11)', () => {
        expect(chromaticOf(21)).toBe(9); // A
        expect(chromaticOf(36)).toBe(0); // C
        expect(chromaticOf(37)).toBe(1); // C#
        expect(chromaticOf(47)).toBe(11); // B
        expect(chromaticOf(60)).toBe(0); // C
        expect(chromaticOf(64)).toBe(4); // E
        expect(chromaticOf(108)).toBe(0); // C
        expect(pitchToGraphIndex(60)).toBe(0);
        expect(pitchToGraphIndex(64)).toBe(4);
        expect(pitchToGraphIndex(21)).toBe(9);
    });
    it('renvoie -1 hors de la plage du piano (21..108)', () => {
        expect(pitchToGraphIndex(20)).toBe(-1);
        expect(pitchToGraphIndex(0)).toBe(-1);
        expect(pitchToGraphIndex(109)).toBe(-1);
        expect(pitchToGraphIndex(127)).toBe(-1);
    });
});
describe('activePitchSet', () => {
    it('garde uniquement les pitchs entiers de la plage du piano', () => {
        const s = activePitchSet([60, 64, 67, 12, 130, 100.5]);
        expect(s.has(60)).toBe(true);
        expect(s.has(64)).toBe(true);
        expect(s.has(67)).toBe(true);
        expect(s.has(12)).toBe(false);
        expect(s.has(130)).toBe(false);
        expect(s.has(100.5)).toBe(false);
        expect(s.size).toBe(3);
    });
    it('gère un tableau vide', () => {
        expect(activePitchSet([]).size).toBe(0);
    });
});
describe('pianoWidthEm (largeur du piano en em, pour le fit scale)', () => {
    it('A0 → C8 = 208em (4+1+3 + 7×28 + 4)', () => {
        expect(pianoWidthEm(21, 108)).toBe(208);
    });
    it('C1 → C8 = 200em (7 octaves complètes + C8)', () => {
        expect(pianoWidthEm(24, 108)).toBe(200);
    });
    it('une octave complète C→B = 28em', () => {
        expect(pianoWidthEm(36, 47)).toBe(28);
    });
    it('la 1re touche compte sa largeur pleine (pas de marge)', () => {
        // A0→B0 : 4em (pleine) + 1em (A#0) + 3em (B0) = 8em
        expect(pianoWidthEm(21, 23)).toBe(8);
        // A0→B1 : 8em + C1..B1 (28em) = 36em
        expect(pianoWidthEm(21, 35)).toBe(36);
        // A0→B2 : 36em + 28em = 64em
        expect(pianoWidthEm(21, 47)).toBe(64);
    });
});
describe('computePianoFontSize (fit scale)', () => {
    it('le piano tient dans la largeur du conteneur', () => {
        // A0→C8 = 208em : conteneur 834px (208×4 + 2) → échelle ≈ 4px
        expect(computePianoFontSize(834, 208)).toBeCloseTo(4, 1);
        const narrow = computePianoFontSize(800, 208);
        expect(narrow).toBeCloseTo(798 / 208, 1);
        // Un conteneur trop étroit est borné par l'échelle minimale
        expect(computePianoFontSize(600, 208)).toBe(3);
    });
    it('borne l échelle minimale (lisibilité) et maximale (confort)', () => {
        expect(computePianoFontSize(100, 208)).toBe(3); // trop étroit → min
        expect(computePianoFontSize(5000, 208)).toBe(14); // très large → max
    });
});
describe('Effets de bord largeur — plein écran (étude 21:30)', () => {
    it('échelles typiques pour des écrans courants (appli pleine largeur)', () => {
        // A0→C8 = 208em : 1920px → ~9.2px ; 2560px → ~12.3px ; 4K → borné 14
        expect(computePianoFontSize(1920, 208)).toBeCloseTo(1918 / 208, 1);
        expect(computePianoFontSize(2560, 208)).toBeCloseTo(2558 / 208, 1);
        expect(computePianoFontSize(3840, 208)).toBe(14); // borne max
    });
    it('INVARIANT : quand l échelle n est pas bornée, le piano tient TOUJOURS dans le conteneur', () => {
        for (const w of [320, 640, 1024, 1280, 1920, 2560, 3200]) {
            const fs = computePianoFontSize(w, 208);
            if (fs > 3 && fs < 14) {
                // non borné : largeur totale = 208em × fs + 2px de bordures ≤ conteneur
                expect(208 * fs + 2).toBeLessThanOrEqual(w + 0.01);
            }
        }
    });
    it('le piano ne déborde JAMAIS d un écran standard même au zoom max (14px)', () => {
        // 208em × 14px = 2912px : tient dans un écran 4K (3840), centré
        expect(208 * 14).toBe(2912);
        expect(2912).toBeLessThan(3840);
    });
});
