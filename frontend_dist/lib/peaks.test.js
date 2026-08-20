/**
 * Tests du calcul de peaks (waveform) — fonction pure, sans Web Audio.
 */
import { describe, expect, it } from 'vitest';
import { computePeaksFromChannels } from './peaks';
const fill = (n, v) => {
    const a = new Float32Array(n);
    a.fill(v);
    return a;
};
describe('computePeaksFromChannels', () => {
    it('signal constant : min === max === la valeur sur chaque bucket', () => {
        const ch = fill(400, 0.5);
        const p = computePeaksFromChannels([ch], 400, 1);
        for (let i = 0; i < p.buckets; i++) {
            expect(p.min[i]).toBeCloseTo(0.5, 5);
            expect(p.max[i]).toBeCloseTo(0.5, 5);
        }
    });
    it('signal alterné 1/-1 : min -1, max 1', () => {
        const ch = new Float32Array(400);
        for (let i = 0; i < 400; i++)
            ch[i] = i % 2 === 0 ? 1 : -1;
        const p = computePeaksFromChannels([ch], 400, 1);
        for (let i = 0; i < p.buckets; i++) {
            expect(p.min[i]).toBe(-1);
            expect(p.max[i]).toBe(1);
        }
    });
    it('nombre de buckets : max(64, durée × bucketsPerSec)', () => {
        expect(computePeaksFromChannels([fill(100, 0)], 100, 1, 60).buckets).toBe(64);
        expect(computePeaksFromChannels([fill(1000, 0)], 1000, 5, 60).buckets).toBe(300);
        expect(computePeaksFromChannels([fill(1000, 0)], 1000, 5, 60).duration).toBe(5);
        expect(computePeaksFromChannels([fill(100, 0)], 100, 1, 60).bucketsPerSec).toBe(60);
    });
    it('stéréo : les peaks couvrent les deux canaux (le max/min des deux)', () => {
        const left = fill(400, 0.1);
        const right = fill(400, 0.9);
        const p = computePeaksFromChannels([left, right], 400, 1);
        for (let i = 0; i < p.buckets; i++) {
            expect(p.max[i]).toBeCloseTo(0.9, 5);
            expect(p.min[i]).toBeCloseTo(0.1, 5);
        }
    });
    it('signal très court : buckets plancher 64, buckets vides = valeurs d init (pas de crash)', () => {
        const ch = new Float32Array(5);
        const p = computePeaksFromChannels([ch], 5, 0.05);
        expect(p.buckets).toBe(64);
        expect(p.min.length).toBe(64);
        expect(p.max.length).toBe(64);
    });
});
