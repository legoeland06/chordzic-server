/**
 * playhead — position de lecture partagée HORS React.
 *
 * Pendant la lecture, la tête de lecture change ~25 fois/seconde. La passer
 * par un state React forcerait un re-render global du DAW à chaque frame.
 * Ce petit store permet de mettre à jour la position SANS re-render (les
 * lignes de lecture se positionnent via `transform`, les abonnés font leur
 * propre setState local quand ils en ont besoin — ex. afficheurs à 10 fps).
 */
let position = 0;
const listeners = new Set();
/** Position courante (beats) — lecture directe sans abonnement. */
export function getPlayheadPosition() {
    return position;
}
/** Met à jour la position et notifie les abonnés (aucun re-render React ici). */
export function setPlayheadPosition(beats) {
    if (beats === position)
        return;
    position = beats;
    for (const fn of listeners)
        fn(beats);
}
/** S'abonne aux changements de position ; retourne la fonction de
 * désabonnement. */
export function subscribePlayhead(fn) {
    listeners.add(fn);
    return () => { listeners.delete(fn); };
}
/** Remet le store à zéro (utile pour les tests). */
export function resetPlayhead() {
    position = 0;
    listeners.clear();
}
