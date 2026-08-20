import { describe, expect, it } from 'vitest';
import { recognizeChord } from './chordRecognition';
import { parseGrille } from '../types/chord';
describe('recognizeChord — reconnaissance d’accords (mode Live)', () => {
    it('aucune note / notes hors plage → null', () => {
        expect(recognizeChord([])).toBeNull();
        expect(recognizeChord([200])).toBeNull();
        expect(recognizeChord([-1])).toBeNull();
    });
    it('une seule note → la note seule, pas un accord', () => {
        const r = recognizeChord([60]);
        expect(r.label).toBe('C');
        expect(r.noteOnly).toBe(true);
        expect(r.insertable).toBe(true);
        expect(recognizeChord([62]).label).toBe('D');
        expect(recognizeChord([61]).label).toBe('C#');
    });
    it('2 notes → match strict uniquement (pas de tolérance)', () => {
        expect(recognizeChord([60, 67]).label).toBe('C5'); // quinte
        expect(recognizeChord([60, 64]).label).toBe('Cno5'); // tierce seule
        // C+F et D+G n'ont qu'un match exact à 2 notes : la quinte relative
        expect(recognizeChord([60, 65]).label).toBe('F5');
        expect(recognizeChord([62, 67]).label).toBe('G5');
    });
    it('2 notes sans accord connu → notes brutes, non insérable', () => {
        const r = recognizeChord([60, 71]); // C + B
        expect(r.label).toBe('C·B');
        expect(r.exact).toBe(false);
        expect(r.insertable).toBe(false);
    });
    it('triades majeures → chiffrage propre sans M', () => {
        expect(recognizeChord([60, 64, 67]).label).toBe('C');
        expect(recognizeChord([61, 65, 68]).label).toBe('C#');
        expect(recognizeChord([62, 66, 69]).label).toBe('D');
    });
    it('triades mineures, diminuées, augmentées, sus', () => {
        expect(recognizeChord([60, 63, 67]).label).toBe('Cm');
        expect(recognizeChord([60, 63, 66]).label).toBe('Cdim');
        expect(recognizeChord([60, 64, 68]).label).toBe('Caug');
        expect(recognizeChord([60, 65, 67]).label).toBe('Csus4');
        expect(recognizeChord([60, 62, 67]).label).toBe('Csus2');
    });
    it('septièmes : C7, CM7, Cm7 (distinction majeure/dominante/mineure)', () => {
        expect(recognizeChord([60, 64, 67, 70]).label).toBe('C7'); // Bb
        expect(recognizeChord([60, 64, 67, 71]).label).toBe('CM7'); // B
        expect(recognizeChord([60, 63, 67, 70]).label).toBe('Cm7');
    });
    it('renversements : la fondamentale est retrouvée (E G C → C)', () => {
        expect(recognizeChord([64, 67, 72]).label).toBe('C'); // 1er renversement
        expect(recognizeChord([67, 72, 76]).label).toBe('C'); // 2e renversement
        expect(recognizeChord([57, 60, 64]).label).toBe('Am'); // A en basse
    });
    it('accords relatifs départagés par la basse réelle (C6 vs Am7)', () => {
        // C E G A — basse C → C6 ; A C E G — basse A → Am7
        expect(recognizeChord([60, 64, 67, 69]).label).toBe('C6');
        expect(recognizeChord([57, 60, 64, 67]).label).toBe('Am7');
    });
    it('doublures et octaves ignorées (même classe)', () => {
        // C3 C4 E4 G4 → C (la doublure du C ne change rien)
        expect(recognizeChord([48, 60, 64, 67]).label).toBe('C');
    });
    it('inclusion à partir de 3 notes : notes ajoutées reconnues (CM9)', () => {
        // C E G B D = CM9 (classes {0,2,4,7,11} — exact)
        expect(recognizeChord([60, 64, 67, 71, 74]).label).toBe('CM9');
    });
    it('inclusion : accord inclus dans des notes supplémentaires', () => {
        // C E G + Bb + F → C7 avec la 11 ajoutée : {0,4,5,7,10}
        // 7sus4 {0,5,7,10} ⊆ (4 incluses, 1 étrangère) ; C7 {0,4,7,10} ⊆ (1 étrangère)
        // Le meilleur score = le plus de notes incluses, le moins d'étrangères :
        // C7 (4 notes, 1 étrangère) = 500+40-1 = 539 ; 7sus4 = 500+40-1 = 539 aussi…
        // L'ordre de la table départage (le premier score > best gagne) : C7 passe avant.
        const r = recognizeChord([60, 64, 67, 70, 77]);
        expect(r.label).toBe('C7');
        expect(r.exact).toBe(false);
        expect(r.insertable).toBe(true);
    });
    it('2 notes strict : pas d’inclusion (C+G+D ne serait pas toléré en 2 notes)', () => {
        // 2 notes : C + G → C5 exact ; jamais un accord à 3 notes
        const r = recognizeChord([60, 67]);
        expect(r.label).toBe('C5');
        expect(r.exact).toBe(true);
    });
    it('l’inclusion ne s’applique qu’à partir de 3 notes', () => {
        // 2 notes C + G : exact → C5 ; si l'inclusion était permise, C (triade)
        // serait candidat — vérifions qu'on reste sur l'exact.
        expect(recognizeChord([60, 67]).label).toBe('C5');
        // 3 notes C G D : sus2 exact (pas un vague C avec inclusion)
        expect(recognizeChord([60, 67, 62]).label).toBe('Csus2');
    });
    it('canal drums : le filtre est côté serveur, la fonction ne reçoit que des pitchs', () => {
        // La fonction est pure : rien à filtrer ici, test documentaire.
        expect(typeof recognizeChord).toBe('function');
    });
});
describe('recognizeChord — convention de basse imposée (harmonie)', () => {
    it('basse grave détachée (≥ 1 octave) : exclue de l’accord, slash si ≠ fondamentale', () => {
        // D2 + C4 E4 G4 → basse D imposée sur C majeur → C/D
        expect(recognizeChord([38, 60, 64, 67]).label).toBe('C/D');
        // G2 + C4 E4 G4 → C/G (la quinte grave n'est PAS une note de l'accord)
        expect(recognizeChord([43, 60, 64, 67]).label).toBe('C/G');
        // E2 + C4 E4 G4 → C/E (l'exemple canonique)
        expect(recognizeChord([40, 60, 64, 67]).label).toBe('C/E');
        // A1 + C4 E4 G4 → C/A
        expect(recognizeChord([33, 60, 64, 67]).label).toBe('C/A');
    });
    it('basse = fondamentale → accord seul, pas de slash', () => {
        // C2 + C4 E4 G4 : la basse confirme la fondamentale → C
        const r = recognizeChord([36, 60, 64, 67]);
        expect(r.label).toBe('C');
        expect(r.bass).toBeNull();
    });
    it('la basse grave n’étend jamais l’accord (pas de 7te/9e imposée)', () => {
        // B1 + C4 E4 G4 → C/B (la septième grave est une basse, pas un CM7)
        expect(recognizeChord([35, 60, 64, 67]).label).toBe('C/B');
        // D2 + C4 E4 G4 B4 → l'accord aigu est CM7, basse D → CM7/D
        expect(recognizeChord([38, 60, 64, 67, 71]).label).toBe('CM7/D');
    });
    it('voicing serré (écart < 1 octave) : pas de basse détachée, renversement normal', () => {
        // E3 G3 C4 → écart 3 : un seul registre → C (renversement, pas C/E)
        expect(recognizeChord([52, 55, 60]).label).toBe('C');
        // C3 G3 → écart 7 : pas de basse → C5
        expect(recognizeChord([48, 55]).label).toBe('C5');
    });
    it('seuil exact : écart 12 = basse détachée, écart 11 = accord serré', () => {
        // C3(48) + C4(60) : écart 12 → basse C == fondamentale → C (sans slash)
        expect(recognizeChord([48, 60, 64, 67]).label).toBe('C');
        // B3(47) + D#4(51) F#4(54) : écart 4 → tout est l'accord → B
        expect(recognizeChord([47, 51, 54]).label).toBe('B');
        // D3(50) + C4(60) E4(64) G4(67) : écart 10 → pas de basse détachée,
        // l'ensemble {D,C,E,G} est lu comme un seul accord
        const r = recognizeChord([50, 60, 64, 67]);
        expect(r.bass).toBeNull();
    });
    it('basse imposée + 1 seule note aiguë → note/basse', () => {
        // C2 + G4 → G/C (la basse diffère de la note)
        expect(recognizeChord([36, 67]).label).toBe('G/C');
        // C2 + C4 → C (la basse confirme la note)
        expect(recognizeChord([36, 60]).label).toBe('C');
    });
    it('l’insertion reste possible : le label avec slash est parseable par la grille', () => {
        const r = recognizeChord([38, 60, 64, 67]); // C/D
        expect(r.insertable).toBe(true);
        // parseGrille accepte la basse alternative : 4:C/D
        const g = parseGrille('4:C/D');
        expect(g.chords[0].chiffrage).toBe('C/D');
        expect(g.chords[0].bass).toBe('D');
    });
});
