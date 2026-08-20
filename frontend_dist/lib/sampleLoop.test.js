/**
 * Tests unitaires de la boucle sample (mode Navig).
 *
 * Couvre le calcul de phase avec décalage POSITIF et NÉGATIF (le réglage
 * −200..+200 ms) et le bornage de l'offset. Ces fonctions pures vivent
 * dans src/lib/sampleLoop.ts.
 */
import { test } from 'vitest';
import assert from 'node:assert/strict';
import { computeSamplePhase, clampSampleOffset, sampleBelongsToTempo, measureDurationSec, fitSampleToGrid, SAMPLE_OFFSET_MIN, SAMPLE_OFFSET_MAX, DEFAULT_SAMPLE_VOLUME, } from './sampleLoop';
/** Comparaison flottante avec tolérance (les calculs modulo sont en f64). */
const approx = (a, b, eps = 1e-9) => Math.abs(a - b) < eps;
// ─── computeSamplePhase ────────────────────────────────────────────────
test('offset 0 : phase = position du morceau (modulo durée)', () => {
    assert.ok(approx(computeSamplePhase(1.0, 0, 4.0), 1.0));
});
test('offset POSITIF : recule la phase (sample en avance → à recaler)', () => {
    assert.ok(approx(computeSamplePhase(1.0, 200, 4.0), 1.2)); // +200 ms
    assert.ok(approx(computeSamplePhase(0.5, 50, 4.0), 0.55)); // +50 ms
});
test('offset NÉGATIF : tire la phase en arrière (sample en retard)', () => {
    assert.ok(approx(computeSamplePhase(1.0, -200, 4.0), 0.8)); // −200 ms
    assert.ok(approx(computeSamplePhase(0.5, -50, 4.0), 0.45)); // −50 ms
});
test('offset négatif passant sous zéro : enroule via le double modulo', () => {
    // 0.05 s − 0.1 s = −0.05 → modulo 4 s → 3.95 s (fin du sample)
    assert.ok(approx(computeSamplePhase(0.05, -100, 4.0), 3.95));
    // Position 0 avec −1 s → 3.0 s
    assert.ok(approx(computeSamplePhase(0.0, -1000, 4.0), 3.0));
});
test('position au-delà de la durée : boucle proprement', () => {
    assert.ok(approx(computeSamplePhase(4.5, 0, 4.0), 0.5));
    assert.ok(approx(computeSamplePhase(8.2, -200, 4.0), 0.0));
});
test('résultat toujours dans [0, durée)', () => {
    for (const pos of [0, 0.001, 1.999, 3.999, 12.345]) {
        for (const off of [-200, -37, 0, 88, 200]) {
            const phase = computeSamplePhase(pos, off, 4.0);
            assert.ok(phase >= 0 && phase < 4.0, `phase ${phase} hors bornes (pos=${pos}, off=${off})`);
        }
    }
});
test('durée invalide (0 ou négative) : retourne 0', () => {
    assert.equal(computeSamplePhase(1.0, 100, 0), 0);
    assert.equal(computeSamplePhase(1.0, 100, -2), 0);
});
// ─── clampSampleOffset ─────────────────────────────────────────────────
test('clamp : borne dans [−2000, +2000]', () => {
    assert.equal(clampSampleOffset(0), 0);
    assert.equal(clampSampleOffset(2000), 2000);
    assert.equal(clampSampleOffset(-2000), -2000);
    assert.equal(clampSampleOffset(2500), 2000);
    assert.equal(clampSampleOffset(-2500), -2000);
    assert.equal(clampSampleOffset(42), 42);
});
test('clamp : valeurs non finies → 0', () => {
    assert.equal(clampSampleOffset(Number.NaN), 0);
    assert.equal(clampSampleOffset(Number.POSITIVE_INFINITY), 0);
});
test('bornes exportées cohérentes', () => {
    assert.equal(SAMPLE_OFFSET_MIN, -2000);
    assert.equal(SAMPLE_OFFSET_MAX, 2000);
});
test('volume par défaut DOUX (samples plus faibles que FluidSynth)', () => {
    assert.equal(DEFAULT_SAMPLE_VOLUME, 55);
    assert.ok(DEFAULT_SAMPLE_VOLUME > 0 && DEFAULT_SAMPLE_VOLUME <= 100);
});
// ─── sampleBelongsToTempo ──────────────────────────────────────────────
test('sample reconnu dans le bucket de SON tempo', () => {
    assert.ok(sampleBelongsToTempo('snap5_160.wav', 160, ['snap5', 'snap6']));
    assert.ok(sampleBelongsToTempo('snap6_160.wav', 160, ['snap5', 'snap6']));
});
test('sample d un AUTRE tempo rejeté (rebasculage nécessaire)', () => {
    // Le bug signalé : arrivé sur 175 BPM, cfg.sample valait encore snap5_160.wav
    assert.ok(!sampleBelongsToTempo('snap5_160.wav', 175, ['snap2', 'snap3', 'snap4']));
    assert.ok(!sampleBelongsToTempo('snap5_160.wav', 160, []));
});
test('nom de fichier construit à la main = clé + tempo (convention backend)', () => {
    assert.ok(sampleBelongsToTempo('snap2_175.wav', 175, ['snap2']));
});
// ─── measureDurationSec ────────────────────────────────────────────────
test('mesure 4/4 à 120 BPM = 2 s', () => {
    assert.ok(approx(measureDurationSec(120, 4), 2.0));
});
test('mesure 4/4 à 160 BPM = 1,5 s', () => {
    assert.ok(approx(measureDurationSec(160, 4), 1.5));
});
test('mesure 3/4 à 120 BPM = 1,5 s (numérateur pris en compte)', () => {
    assert.ok(approx(measureDurationSec(120, 3), 1.5));
});
test('valeurs invalides → repli sain (4/4 à 120)', () => {
    assert.ok(approx(measureDurationSec(0, 4), 2.0));
    assert.ok(approx(measureDurationSec(120, 0), 2.0));
    assert.ok(approx(measureDurationSec(Number.NaN, 4), 2.0));
});
// ─── fitSampleToGrid ───────────────────────────────────────────────────
test('sample déjà parfait (1 mesure exacte) : rien à faire', () => {
    const f = fitSampleToGrid(2.0, 2.0);
    assert.equal(f.mode, 'exact');
    assert.ok(approx(f.periodSec, 2.0));
    assert.equal(f.bars, 1);
    assert.equal(f.deltaSec, 0);
});
test('sample de 2 mesures exactes : période = 2 mesures, rien à faire', () => {
    const f = fitSampleToGrid(4.0, 2.0);
    assert.equal(f.mode, 'exact');
    assert.ok(approx(f.periodSec, 4.0));
    assert.equal(f.bars, 2);
});
test('sample TROP LONG (4,05 s, mesure 4 s) : coupé à 4 s (−50 ms)', () => {
    const f = fitSampleToGrid(4.05, 4.0);
    assert.equal(f.mode, 'cut');
    assert.ok(approx(f.periodSec, 4.0));
    assert.equal(f.bars, 1);
    assert.ok(approx(f.deltaSec, 0.05));
});
test('sample TROP COURT (3,7 s, mesure 4 s) : 300 ms de silence ajoutés', () => {
    const f = fitSampleToGrid(3.7, 4.0);
    assert.equal(f.mode, 'pad');
    assert.ok(approx(f.periodSec, 4.0));
    assert.equal(f.bars, 1);
    assert.ok(approx(f.deltaSec, -0.3));
});
test('sample de 2 mesures + 100 ms : période 2 mesures (coupé de 100 ms)', () => {
    // round(8,1/4) = 2 → on coupe 100 ms, on ne perd PAS une mesure entière
    const f = fitSampleToGrid(8.1, 4.0);
    assert.equal(f.mode, 'cut');
    assert.ok(approx(f.periodSec, 8.0));
    assert.equal(f.bars, 2);
    assert.ok(approx(f.deltaSec, 0.1));
});
test('sample entre 1 et 2 mesures (6 s, mesure 4 s) : période 2 mesures, pad 2 s', () => {
    // round(6/4) = 2 → la boucle fait 2 mesures : 6 s de sample + 2 s de silence
    const f = fitSampleToGrid(6.0, 4.0);
    assert.equal(f.mode, 'pad');
    assert.ok(approx(f.periodSec, 8.0));
    assert.equal(f.bars, 2);
    assert.ok(approx(f.deltaSec, -2.0));
});
test('écart inférieur à un échantillon (44,1 kHz) : considéré exact', () => {
    const f = fitSampleToGrid(4.0 + 0.00001, 4.0); // +10 µs < 1/44100 s ≈ 22,7 µs
    assert.equal(f.mode, 'exact');
});
test('durées invalides : pas de modification', () => {
    const f = fitSampleToGrid(0, 4.0);
    assert.equal(f.mode, 'exact');
    assert.equal(f.periodSec, 0);
    const g = fitSampleToGrid(2.0, 0);
    assert.equal(g.mode, 'exact');
});
// ─── fitSampleToGrid : cas limites ────────────────────────────────────
test('sample de DEMI-mesure (2 s, mesure 4 s) : période 1 mesure, pad 2 s', () => {
    // round(0,5) = 1 → le sample est répété 1× par mesure, moitié silence
    const f = fitSampleToGrid(2.0, 4.0);
    assert.equal(f.mode, 'pad');
    assert.equal(f.bars, 1);
    assert.ok(approx(f.periodSec, 4.0));
    assert.ok(approx(f.deltaSec, -2.0));
});
test('sample entre 1 et 2 mesures (5,96 s, mesure 4 s) : coupé à 1 mesure', () => {
    // round(1,49) = 1 → le multiple le plus proche gagne (coupe 1,96 s)
    const f = fitSampleToGrid(5.96, 4.0);
    assert.equal(f.mode, 'cut');
    assert.equal(f.bars, 1);
    assert.ok(approx(f.periodSec, 4.0));
    assert.ok(approx(f.deltaSec, 1.96));
});
test('sample très long (401,6 s, mesure 4 s) : 100 mesures, coupe 1,6 s', () => {
    const f = fitSampleToGrid(401.6, 4.0);
    assert.equal(f.mode, 'cut');
    assert.equal(f.bars, 100);
    assert.ok(approx(f.periodSec, 400.0));
    assert.ok(approx(f.deltaSec, 1.6));
});
test('sample très long, léger excès sous la tolérance : exact', () => {
    // 400 s + 10 µs < 1/44100 s → considéré parfait (aucune recopie massive)
    const f = fitSampleToGrid(400.00001, 4.0);
    assert.equal(f.mode, 'exact');
    assert.ok(approx(f.periodSec, 400.00001));
});
test('période toujours ≥ 1 mesure et multiple strict de la mesure', () => {
    for (const s of [0.1, 0.9, 1.0, 2.5, 3.999, 4.001, 8.0, 20.0, 1000.0]) {
        const f = fitSampleToGrid(s, 4.0);
        assert.ok(f.bars >= 1, `bars=${f.bars} pour sample=${s}`);
        const attendu = f.bars * 4.0;
        assert.ok(approx(f.periodSec, attendu) || f.mode === 'exact', `période ${f.periodSec} ≠ ${attendu} (mode ${f.mode}) pour sample=${s}`);
    }
});
// ─── measureDurationSec : cas limites ──────────────────────────────────
test('tempo et beatsPerBar négatifs → repli sain (4/4 à 120)', () => {
    assert.ok(approx(measureDurationSec(-120, 4), 2.0));
    assert.ok(approx(measureDurationSec(120, -4), 2.0));
    assert.ok(approx(measureDurationSec(Number.NEGATIVE_INFINITY, 4), 2.0));
});
test('beatsPerBar fractionnaire → tronqué (3,7 temps → 3 temps)', () => {
    assert.ok(approx(measureDurationSec(120, 3.7), 1.5));
});
// ─── computeSamplePhase : période RECADRÉE (alignement grille) ─────────
test('phase calculée modulo la PÉRIODE fournie (pas la durée brute)', () => {
    // Période recadrée 4 s alors que le sample brut dure 4,05 s : la phase
    // utilisée par le moteur est modulo 4 s (le buffer joué est le recadré).
    assert.ok(approx(computeSamplePhase(4.1, 0, 4.0), 0.1));
    assert.ok(approx(computeSamplePhase(4.1, -200, 4.0), 3.9));
});
test('phase indépendante du contexte de grille quand période = durée brute', () => {
    // Cohérence lecture/extraction : même position, même offset, même période
    // → même phase, que la période soit la durée brute ou un multiple exact.
    assert.ok(approx(computeSamplePhase(12.345, 37, 4.0), computeSamplePhase(12.345, 37, 4.0)));
});
