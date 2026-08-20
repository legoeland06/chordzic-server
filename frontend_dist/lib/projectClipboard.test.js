/**
 * Tests du presse-papiers de projet (copier/coller entre pistes).
 */
import { beforeEach, describe, expect, it } from 'vitest';
import { getProjectClipboard, setProjectClipboard, subscribeProjectClipboard, } from './projectClipboard';
const clip = (notes, extra) => ({
    notes,
    minStart: 0,
    sourceChannel: 0,
    sourceLabel: 'Lead',
    wholeTrack: false,
    copiedAt: Date.now(),
    ...extra,
});
beforeEach(() => setProjectClipboard(null));
describe('projectClipboard (singleton partagé entre pistes)', () => {
    it('vide par défaut', () => {
        expect(getProjectClipboard()).toBeNull();
    });
    it('set puis get retourne le contenu', () => {
        const c = clip([{ id: 'n1', startTime: 0, pitch: 60, duration: 1, velocity: 100 }]);
        setProjectClipboard(c);
        expect(getProjectClipboard()).toBe(c);
    });
    it('set(null) vide le presse-papiers', () => {
        setProjectClipboard(clip([]));
        setProjectClipboard(null);
        expect(getProjectClipboard()).toBeNull();
    });
    it('notifie les souscripteurs à chaque changement', () => {
        let calls = 0;
        const unsub = subscribeProjectClipboard(() => { calls += 1; });
        setProjectClipboard(clip([]));
        setProjectClipboard(clip([], { wholeTrack: true }));
        setProjectClipboard(null);
        expect(calls).toBe(3);
        unsub();
        setProjectClipboard(clip([]));
        expect(calls).toBe(3); // plus notifié après désabonnement
    });
});
