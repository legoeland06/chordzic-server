/**
 * Helpers purs de position de lecture — mode Navig.
 *
 * En mode « Sortie » (clic séparé, lecture SERVEUR en double canaux), le
 * navigateur ne joue aucun buffer : il n'a donc pas d'horloge audio. La tête
 * de lecture est alors estimée localement (performance.now) — ces fonctions
 * pures rendent cette logique testable (aucune dépendance DOM/audio).
 */
/** Position estimée (secondes) depuis le début de la lecture serveur.
 * `startMs` : performance.now() au démarrage ; `nowMs` : maintenant. */
export function estimatePositionSec(startMs, nowMs) {
    return Math.max(0, (nowMs - startMs) / 1000);
}
/** Beats → secondes (tempo en BPM). */
export function secondsFromBeats(beats, tempo) {
    return (beats * 60) / Math.max(1, tempo);
}
/** Secondes → beats (tempo en BPM). */
export function beatsFromSeconds(sec, tempo) {
    return (Math.max(0, sec) * Math.max(1, tempo)) / 60;
}
/** start_at (beats) à envoyer au backend pour une lecture qui doit démarrer
 * à `seconds` — utilisé par le scrub en mode séparé (navig-play). */
export function navStartAtBeats(seconds, tempo) {
    return beatsFromSeconds(seconds, tempo);
}
/** Position de tête dans une boucle [loopStartSec, loopEndSec[ : wrap la
 * position linéaire dans l'intervalle (même comportement que la lecture
 * serveur et le loop Web Audio). Tant que la position n'a pas atteint le
 * locator droit, elle reste telle quelle (la lecture peut commencer avant
 * L — le 1ᵉʳ passage joue de start à R, puis la boucle [L, R[). */
export function wrapLoopPositionSec(sec, loopStartSec, loopEndSec) {
    if (!(loopEndSec > loopStartSec))
        return sec;
    if (sec < loopStartSec)
        return sec;
    const len = loopEndSec - loopStartSec;
    if (len <= 0)
        return sec;
    return loopStartSec + ((sec - loopStartSec) % len);
}
/** Position de départ de la lecture quand le repeat boucle [L, R[ :
 * - tête AU-DELÀ de R (ou égale) → retour au locator gauche (la lecture
 *   tomberait dans le vide) ;
 * - tête AVANT L (dont 0 par défaut) → on joue depuis la tête : un
 *   premier Play avec L ≠ 0 doit jouer le morceau depuis le début, pas
 *   sauter le début (bug : « les modifications au début ne sont pas
 *   entendues » quand le locator gauche n'est pas à 001.1) ;
 * - tête dans [L, R[ → la tête. */
export function computeStartBeats(loopOn, locL, locR, posBeats) {
    if (loopOn && locR > locL && posBeats >= locR)
        return locL;
    return posBeats;
}
/** Beats → format « MMM.T » (mesure.temps) — même affichage que le compteur
 * MES de la barre de transport. Cohérent avec la signature (temps/mesure). */
export function locBeatToMes(beat, beatsPerBar) {
    const b = Math.max(0, Math.floor(beat));
    const m = Math.floor(b / beatsPerBar) + 1;
    const t = (b % beatsPerBar) + 1;
    return `${String(m).padStart(3, '0')}.${t}`;
}
/** « MMM.T » → beats (null si invalide ou temps hors mesure). */
export function locMesToBeat(text, beatsPerBar) {
    const m = text.trim().match(/^(\d{1,4})\.(\d{1,2})$/);
    if (!m)
        return null;
    const mes = parseInt(m[1], 10);
    const t = parseInt(m[2], 10);
    if (mes < 1 || t < 1 || t > beatsPerBar)
        return null;
    return (mes - 1) * beatsPerBar + (t - 1);
}
// ─── Alignement vertical des pistes (mode Navig) ──────────────────────
/** Hauteur d'une ligne de piste (lane compacte + sa bordure/bas). */
export function laneRowHeight(compactLaneH, gap) {
    return compactLaneH + gap;
}
/** Top Y d'une lane dans la colonne de CONTENU : la barre des locators
 * (hauteur headerH) est au-dessus — les lanes commencent dessous. */
export function laneTop(index, compactLaneH, gap, headerH) {
    return headerH + index * laneRowHeight(compactLaneH, gap);
}
/** Top Y d'un NOM de piste dans le panneau gauche (sans le header). Le
 * panneau doit être décalé de headerH en tête pour que nom et lane restent
 * ALIGNÉS : nameTop(i) + headerH === laneTop(i). */
export function nameTop(index, compactLaneH, gap) {
    return index * laneRowHeight(compactLaneH, gap);
}
