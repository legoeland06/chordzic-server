/**
 * projectClipboard — presse-papiers GLOBAL du projet (partagé entre les
 * Piano Rolls des différentes pistes).
 *
 * Permet de copier les notes d'une piste (piste_Origine) et de les coller
 * dans une autre piste (piste_Destination) aux MÊMES emplacements et
 * valeurs (startTime, pitch, duration, velocity inchangés).
 *
 * Singleton en mémoire + petit système de souscription pour que les
 * boutons de l'UI (Copier / Coller) reflètent l'état du presse-papiers.
 */
let current = null;
/** Souscripteurs (fonctions appelées à chaque changement). */
const listeners = new Set();
function notify() {
    for (const fn of listeners)
        fn();
}
/** Contenu courant du presse-papiers (null = vide). */
export function getProjectClipboard() {
    return current;
}
/** Remplace le contenu du presse-papiers (null = vider). */
export function setProjectClipboard(clip) {
    current = clip;
    notify();
}
/** S'abonne aux changements ; retourne la fonction de désabonnement. */
export function subscribeProjectClipboard(fn) {
    listeners.add(fn);
    return () => { listeners.delete(fn); };
}
