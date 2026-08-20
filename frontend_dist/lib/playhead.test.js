/**
 * Tests du store de position de lecture (playhead).
 */
import { beforeEach, describe, expect, it } from 'vitest';
import { getPlayheadPosition, resetPlayhead, setPlayheadPosition, subscribePlayhead, } from './playhead';
beforeEach(() => resetPlayhead());
describe('playhead (position de lecture hors React)', () => {
    it('position initiale à 0', () => {
        expect(getPlayheadPosition()).toBe(0);
    });
    it('set puis get', () => {
        setPlayheadPosition(12.5);
        expect(getPlayheadPosition()).toBe(12.5);
    });
    it('notifie les abonnés (avec la valeur)', () => {
        const seen = [];
        const unsub = subscribePlayhead(b => seen.push(b));
        setPlayheadPosition(4);
        setPlayheadPosition(8);
        expect(seen).toEqual([4, 8]);
        unsub();
        setPlayheadPosition(16);
        expect(seen).toEqual([4, 8]); // plus notifié après désabonnement
    });
    it('n émet pas si la position est identique (anti-spam)', () => {
        let calls = 0;
        subscribePlayhead(() => { calls += 1; });
        setPlayheadPosition(5);
        setPlayheadPosition(5);
        expect(calls).toBe(1);
    });
    it('plusieurs abonnés', () => {
        let a = 0;
        let b = 0;
        subscribePlayhead(() => { a += 1; });
        subscribePlayhead(() => { b += 1; });
        setPlayheadPosition(1);
        expect(a).toBe(1);
        expect(b).toBe(1);
    });
});
