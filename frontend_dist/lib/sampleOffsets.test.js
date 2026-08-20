/**
 * Tests du module sampleOffsets (décalages mémorisés PAR sample).
 * Le localStorage est simulé (vitest, environnement node).
 */
import { test, vi, beforeEach } from 'vitest';
import assert from 'node:assert/strict';
import { loadSampleOffsets, saveSampleOffsets } from './sampleOffsets';
const storage = new Map();
beforeEach(() => {
    storage.clear();
    vi.stubGlobal('localStorage', {
        getItem: (k) => storage.get(k) ?? null,
        setItem: (k, v) => { storage.set(k, v); },
        removeItem: (k) => { storage.delete(k); },
    });
});
test('aucune préférence enregistrée → {}', () => {
    assert.deepEqual(loadSampleOffsets(), {});
});
test('save puis load : la valeur par sample est retrouvée', () => {
    saveSampleOffsets({ 'snap5_160.wav': 25, 'snap6_160.wav': -12 });
    const loaded = loadSampleOffsets();
    assert.equal(loaded['snap5_160.wav'], 25);
    assert.equal(loaded['snap6_160.wav'], -12);
});
test('les valeurs NÉGATIVES sont conservées', () => {
    saveSampleOffsets({ 'snap2_175.wav': -35 });
    assert.equal(loadSampleOffsets()['snap2_175.wav'], -35);
});
test('écrasement : le dernier verrouillage gagne pour un sample donné', () => {
    saveSampleOffsets({ 'snap5_160.wav': 10 });
    saveSampleOffsets({ 'snap5_160.wav': 42, 'snap7_150.wav': 0 });
    assert.equal(loadSampleOffsets()['snap5_160.wav'], 42);
});
test('JSON corrompu → {} sans planter', () => {
    storage.set('chordzic_sample_offsets_v1', '{pas du json');
    assert.deepEqual(loadSampleOffsets(), {});
});
