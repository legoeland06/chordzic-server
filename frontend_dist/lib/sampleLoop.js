/**
 * sampleLoop.ts — fonctions pures de la boucle sample (mode Navig).
 *
 * Séparées du moteur Web Audio pour être testables unitairement :
 *  - calcul de phase avec décalage NÉGATIF (double modulo) ;
 *  - alignement sur la grille : durée d'une mesure, période de boucle
 *    « parfaite » (multiple entier de la mesure — coupe ou silence).
 */
/** Borne du décalage de phase (ms) : −200..+200. */
export const SAMPLE_OFFSET_MIN = -2000;
export const SAMPLE_OFFSET_MAX = 2000;
/** Volume par défaut du sample (0-100) — volontairement DOUX (55 ≈ −3 dB
 * vs 80) : les samples bruts sont souvent plus forts que le rendu
 * FluidSynth. Le slider reste disponible en aval pour ajuster. */
export const DEFAULT_SAMPLE_VOLUME = 55;
/** Vrai si le sample (nom complet, ex. « snap5_160.wav ») appartient au
 * bucket de clés du tempo donné — utilisé pour rebasculer automatiquement
 * le sample quand on CHANGE de tempo. */
export function sampleBelongsToTempo(sample, tempo, bucketKeys) {
    return bucketKeys.some((s) => sample === `${s}_${tempo}.wav`);
}
/** Borne une valeur de décalage dans [MIN, MAX]. */
export function clampSampleOffset(ms) {
    if (!Number.isFinite(ms))
        return 0;
    return Math.max(SAMPLE_OFFSET_MIN, Math.min(SAMPLE_OFFSET_MAX, ms));
}
/**
 * Position de lecture dans le sample (secondes) pour la position courante
 * du morceau et un décalage de phase donné.
 *
 * `phase = (position_du_morceau + décalage) mod durée_de_la_période`
 * — le double modulo garantit un résultat dans [0, durée) même pour un
 * décalage NÉGATIF (le sample est tiré en arrière dans le temps).
 * `durationSec` est la PÉRIODE de boucle effective : durée brute du sample
 * s'il est déjà aligné, ou période recadrée (voir fitSampleToGrid) sinon.
 */
export function computeSamplePhase(positionSec, offsetMs, durationSec) {
    if (!(durationSec > 0))
        return 0;
    const shifted = positionSec + offsetMs / 1000;
    let phase = ((shifted % durationSec) + durationSec) % durationSec;
    // Robustesse flottante : un résultat à ε près de la durée (ex. 3,9999…
    // pour 4 s) est en réalité le DÉBUT du sample — le normaliser à 0 évite
    // de jouer un échantillon quasi vide en fin de buffer.
    const eps = durationSec * 1e-12;
    if (phase < eps || phase > durationSec - eps)
        phase = 0;
    return phase;
}
// ─── Alignement du sample sur la grille (mesures) ────────────────────────
//
// Le sample est répété en boucle pour couvrir tout le morceau. Pour qu'il ne
// DÉRIVE PAS par rapport au métronome (à chaque boucle, la phase se décale
// de `durée_sample − durée_de_la_période`), la période de boucle doit être un
// MULTIPLE ENTIER de la durée d'une mesure. Si le sample n'est pas déjà
// parfait, on l'ajuste automatiquement :
//  - trop long  → coupé à la période cible ;
//  - trop court → complété par du silence (espace entre chaque répétition).
/** Durée d'UNE mesure en secondes (ex. 4/4 à 120 BPM → 2 s). */
export function measureDurationSec(tempo, beatsPerBar) {
    const bpb = Number.isFinite(beatsPerBar) && beatsPerBar >= 1 ? Math.floor(beatsPerBar) : 4;
    const t = Number.isFinite(tempo) && tempo > 0 ? tempo : 120;
    return (bpb * 60) / t;
}
/** Tolérance d'ajustement (s) : en dessous, le sample est considéré déjà
 * aligné (~1 échantillon à 44,1 kHz — inaudible, on ne touche à rien). */
export const SAMPLE_GRID_EPS_SEC = 1 / 44100;
/** Calcule la période de boucle « parfaite » pour un sample : le multiple
 * entier de la mesure le plus proche de sa durée réelle (1 mesure minimum).
 * Ex. : sample 4,05 s, mesure 4 s → période 4 s (coupé de 50 ms) ;
 * sample 3,7 s, mesure 4 s → période 4 s (300 ms de silence ajoutés). */
export function fitSampleToGrid(sampleDurationSec, measureSec) {
    if (!(sampleDurationSec > 0) || !(measureSec > 0)) {
        return { periodSec: Math.max(0, sampleDurationSec), bars: 1, mode: 'exact', deltaSec: 0 };
    }
    const bars = Math.max(1, Math.round(sampleDurationSec / measureSec));
    const periodSec = bars * measureSec;
    const deltaSec = sampleDurationSec - periodSec;
    if (Math.abs(deltaSec) < SAMPLE_GRID_EPS_SEC) {
        return { periodSec: sampleDurationSec, bars, mode: 'exact', deltaSec: 0 };
    }
    return { periodSec, bars, mode: deltaSec > 0 ? 'cut' : 'pad', deltaSec };
}
