import { describe, expect, it } from 'vitest';
import { durationLabel, parseChord, parseGrille, parseRepeat } from '../types/chord';
describe('parseChord — chiffrage d’affichage propre', () => {
    it('plus de « / » quand aucune basse alternative n’est spécifiée', () => {
        expect(parseChord('2:G7').chiffrage).toBe('G7'); // avant : "G7/"
        expect(parseChord('4:Cm7').chiffrage).toBe('Cm7');
        expect(parseChord('2:FM7').chiffrage).toBe('FM7');
    });
    it('le « / » reste pour une basse alternative explicite', () => {
        expect(parseChord('1:Am7/D').chiffrage).toBe('Am7/D');
        expect(parseChord('4:C7/Bb').chiffrage).toBe('C7/Bb');
        expect(parseChord('4:G/B').chiffrage).toBe('G/B');
    });
    it('les triades majeures s’affichent sans suffixe M', () => {
        expect(parseChord('4:C').chiffrage).toBe('C'); // avant : "CM"
        expect(parseChord('4:CM').chiffrage).toBe('C');
        expect(parseChord('4:Cmaj').chiffrage).toBe('C');
        expect(parseChord('4:C#').chiffrage).toBe('C#');
        expect(parseChord('4:Db').chiffrage).toBe('Db');
    });
    it('les extensions majeures gardent leur suffixe (M7, M9…)', () => {
        expect(parseChord('4:CM7').chiffrage).toBe('CM7');
        expect(parseChord('4:CM9').chiffrage).toBe('CM9');
        expect(parseChord('4:Cmaj7').chiffrage).toBe('Cmaj7');
    });
    it('les autres qualités restent intactes', () => {
        expect(parseChord('4:Cm7').chiffrage).toBe('Cm7');
        expect(parseChord('4:Cdim').chiffrage).toBe('Cdim');
        expect(parseChord('4:Csus4').chiffrage).toBe('Csus4');
        expect(parseChord('4:C7#9').chiffrage).toBe('C7#9');
    });
    it('silence : chiffrage "_" inchangé', () => {
        expect(parseChord('2:_').chiffrage).toBe('_');
        expect(parseChord('2:_').notes).toEqual([]);
    });
    it('la durée et les notes MIDI restent corrects (aucune régression)', () => {
        const c = parseChord('2:G7');
        expect(c.time).toBe(2);
        expect(c.midiValues).toEqual([7, 11, 14, 17]); // G B D F
        expect(c.notes).toEqual(['G', 'B', 'D', 'F']);
        const c2 = parseChord('4:Cm');
        expect(c2.time).toBe(4);
        expect(c2.midiValues).toEqual([0, 3, 7]); // C Eb G
        // Sans durée : défaut 4
        expect(parseChord('G7').time).toBe(4);
        // Basse alternative résolue : bassNote = basse, notes MIDI absolues
        const c3 = parseChord('1:Am7/D');
        expect(c3.bass).toBe('D');
        expect(c3.midiValues).toEqual([9, 12, 16, 19]); // A C E G (MIDI absolu)
    });
    it('token invalide → exception', () => {
        expect(() => parseChord('zzz')).toThrow();
        expect(() => parseChord('')).toThrow();
    });
});
describe('parseRepeat — notation de répétition xN', () => {
    it('extrait le facteur et la base', () => {
        expect(parseRepeat('2:Cm7x3')).toEqual({ base: '2:Cm7', repeat: 3 });
        expect(parseRepeat('4:_x2')).toEqual({ base: '4:_', repeat: 2 });
        expect(parseRepeat('1:Am7/Dx4')).toEqual({ base: '1:Am7/D', repeat: 4 });
    });
    it('sans xN : repeat 1, base inchangée', () => {
        expect(parseRepeat('2:Cm7')).toEqual({ base: '2:Cm7', repeat: 1 });
        expect(parseRepeat('4:C')).toEqual({ base: '4:C', repeat: 1 });
    });
    it('x1 = 1 fois, x0 clampé à 1 (jamais d’accord qui disparaît)', () => {
        expect(parseRepeat('2:Cm7x1')).toEqual({ base: '2:Cm7', repeat: 1 });
        expect(parseRepeat('2:Cm7x0')).toEqual({ base: '2:Cm7', repeat: 1 });
    });
    it('gros facteurs acceptés (x16, x32…)', () => {
        expect(parseRepeat('4:Cx16').repeat).toBe(16);
        expect(parseRepeat('1:Amx32').repeat).toBe(32);
    });
});
describe('parseGrille — expansion des répétitions', () => {
    it('2:Cm7x3 → 3 accords Cm7 de 2 temps', () => {
        const g = parseGrille('2:Cm7x3');
        expect(g.chords).toHaveLength(3);
        for (const c of g.chords) {
            expect(c.chiffrage).toBe('Cm7');
            expect(c.time).toBe(2);
        }
    });
    it('mélange notations normales et xN', () => {
        const g = parseGrille('4:C 2:G7x2 1:Am7/D');
        expect(g.chords).toHaveLength(4);
        expect(g.chords.map(c => c.chiffrage)).toEqual(['C', 'G7', 'G7', 'Am7/D']);
        expect(g.chords.map(c => c.time)).toEqual([4, 2, 2, 1]);
    });
    it('répétition de silences : 4:_x2 → 2 silences', () => {
        const g = parseGrille('4:_x2 4:C');
        expect(g.chords).toHaveLength(3);
        expect(g.chords[0].chiffrage).toBe('_');
        expect(g.chords[1].chiffrage).toBe('_');
        expect(g.chords[2].chiffrage).toBe('C');
    });
    it('les occurrences sont indépendantes (copies, pas de références partagées)', () => {
        const g = parseGrille('2:Cm7x3');
        expect(g.chords[0]).not.toBe(g.chords[1]);
        expect(g.chords[0].notes).not.toBe(g.chords[1].notes);
        g.chords[0].midiValues.push(99);
        expect(g.chords[1].midiValues).toEqual([0, 3, 7, 10]);
    });
    it('grille vide → exception (comportement historique, gardé en amont) ; token invalide → exception', () => {
        expect(() => parseGrille('')).toThrow();
        expect(() => parseGrille('zzz')).toThrow();
    });
    it('xN pris en compte dans la durée totale (beats)', () => {
        const beats = (input) => {
            let sum = 0;
            for (const tok of input.trim().split(/\s+/)) {
                const { base, repeat } = parseRepeat(tok);
                const m = base.match(/^(\d+):/);
                if (m)
                    sum += (4 / parseInt(m[1], 10)) * repeat;
            }
            return sum;
        };
        // 2:Cm7x3 = 3 × 2 temps = 6 beats (avant : 2 beats — la régression que
        // le totalBeats de DawView aurait eue sans la prise en charge de xN)
        expect(beats('2:Cm7x3')).toBe(6);
        expect(beats('4:C 2:G7x2')).toBe(1 + 2 + 2); // 5
    });
});
describe('durationLabel — figures rythmiques des mentions discrètes', () => {
    it('table complète', () => {
        expect(durationLabel(1)).toBe('ronde');
        expect(durationLabel(2)).toBe('blanche');
        expect(durationLabel(3)).toBe('3 par mesure');
        expect(durationLabel(4)).toBe('noire');
        expect(durationLabel(6)).toBe('triolet de noire');
        expect(durationLabel(8)).toBe('croche');
        expect(durationLabel(12)).toBe('triolet de croche');
        expect(durationLabel(16)).toBe('double croche');
        expect(durationLabel(24)).toBe('sextolet de croche');
        expect(durationLabel(32)).toBe('triple croche');
        expect(durationLabel(64)).toBe('quadruple croche');
    });
    it('valeurs sans figure standard → « N par mesure »', () => {
        expect(durationLabel(5)).toBe('5 par mesure');
        expect(durationLabel(7)).toBe('7 par mesure');
    });
});
