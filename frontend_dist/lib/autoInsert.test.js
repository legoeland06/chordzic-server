import { describe, expect, it } from 'vitest';
import { computeAutoInsert, initialAutoInsertState } from './autoInsert';
const T0 = 1000000;
describe('computeAutoInsert — timer d’insertion automatique', () => {
    it('état initial : un accord tenu arme le timer, rien inséré', () => {
        const s = initialAutoInsertState();
        const r = computeAutoInsert(s, T0, 2000, 'C|0,4,7', true);
        expect(r.shouldInsert).toBe(false);
        expect(r.next.firstSeenAt).toBe(T0);
        expect(r.next.lastKey).toBeNull();
    });
    it('même accord tenu pendant le délai → insertion une seule fois', () => {
        let s = initialAutoInsertState();
        let r = computeAutoInsert(s, T0, 2000, 'C|0,4,7', true);
        s = r.next;
        // 1 s : pas encore
        r = computeAutoInsert(s, T0 + 1000, 2000, 'C|0,4,7', true);
        expect(r.shouldInsert).toBe(false);
        s = r.next;
        // 2 s : insertion
        r = computeAutoInsert(s, T0 + 2000, 2000, 'C|0,4,7', true);
        expect(r.shouldInsert).toBe(true);
        expect(r.next.lastKey).toBe('C|0,4,7');
        s = r.next;
        // Toujours tenu : pas de doublon
        r = computeAutoInsert(s, T0 + 4000, 2000, 'C|0,4,7', true);
        expect(r.shouldInsert).toBe(false);
    });
    it('changement d’accord sans relâcher → le timer redémarre', () => {
        let s = initialAutoInsertState();
        let r = computeAutoInsert(s, T0, 2000, 'C|0,4,7', true);
        s = r.next;
        r = computeAutoInsert(s, T0 + 2500, 2000, 'C|0,4,7', true);
        expect(r.shouldInsert).toBe(true); // C inséré après 2,5 s
        s = r.next;
        // Changement direct C → G (notes modifiées sans relâche)
        r = computeAutoInsert(s, T0 + 2600, 2000, 'G|7,11,14', true);
        expect(r.shouldInsert).toBe(false); // G n'a été tenu que 100 ms
        expect(r.next.firstSeenAt).toBe(T0 + 2600);
        s = r.next;
        r = computeAutoInsert(s, T0 + 4700, 2000, 'G|7,11,14', true);
        expect(r.shouldInsert).toBe(true); // G tenu 2,1 s → inséré
    });
    it('relâche complète → le même accord peut être réinséré plus tard', () => {
        let s = initialAutoInsertState();
        let r = computeAutoInsert(s, T0, 2000, 'C|0,4,7', true);
        s = r.next;
        r = computeAutoInsert(s, T0 + 2000, 2000, 'C|0,4,7', true);
        expect(r.shouldInsert).toBe(true);
        s = r.next;
        // Relâche
        r = computeAutoInsert(s, T0 + 2500, 2000, null, false);
        expect(r.next.lastKey).toBeNull();
        expect(r.next.firstSeenAt).toBeNull();
        s = r.next;
        // Rejoue le même C plus tard → réinséré après le délai
        r = computeAutoInsert(s, T0 + 3000, 2000, 'C|0,4,7', true);
        expect(r.shouldInsert).toBe(false);
        s = r.next;
        r = computeAutoInsert(s, T0 + 5200, 2000, 'C|0,4,7', true);
        expect(r.shouldInsert).toBe(true);
    });
    it('notes brutes (non identifiées) : le timer ne s’arme pas', () => {
        let s = initialAutoInsertState();
        const r = computeAutoInsert(s, T0, 2000, 'C·B|0,11', false);
        expect(r.shouldInsert).toBe(false);
        expect(r.next.firstSeenAt).toBeNull();
    });
    it('délai différent (1 s, 5 s) respecté', () => {
        let s = initialAutoInsertState();
        let r = computeAutoInsert(s, T0, 1000, 'C|0,4,7', true);
        s = r.next;
        r = computeAutoInsert(s, T0 + 999, 1000, 'C|0,4,7', true);
        expect(r.shouldInsert).toBe(false);
        r = computeAutoInsert(s, T0 + 1000, 1000, 'C|0,4,7', true);
        expect(r.shouldInsert).toBe(true);
        // 5 s
        s = initialAutoInsertState();
        r = computeAutoInsert(s, T0, 5000, 'C|0,4,7', true);
        s = r.next;
        r = computeAutoInsert(s, T0 + 4999, 5000, 'C|0,4,7', true);
        expect(r.shouldInsert).toBe(false);
        r = computeAutoInsert(s, T0 + 5000, 5000, 'C|0,4,7', true);
        expect(r.shouldInsert).toBe(true);
    });
});
