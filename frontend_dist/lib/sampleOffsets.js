/**
 * sampleOffsets.ts — préférences de décalage MÉMORISÉES par sample.
 *
 * Chaque sample peut garder son propre décalage de phase (validé par
 * l'utilisateur via le verrou 🔒 du LoopControl). Stockage GLOBAL
 * (localStorage) : la préférence suit le sample, pas le projet.
 */
const KEY = 'chordzic_sample_offsets_v1';
/** Charge le map « nom de fichier sample → offset ms » ({} si rien). */
export function loadSampleOffsets() {
    try {
        const raw = localStorage.getItem(KEY);
        if (raw) {
            const parsed = JSON.parse(raw);
            if (parsed && typeof parsed === 'object')
                return parsed;
        }
    }
    catch {
        /* localStorage indisponible ou corrompu */
    }
    return {};
}
/** Sauvegarde le map complet des offsets mémorisés. */
export function saveSampleOffsets(map) {
    try {
        localStorage.setItem(KEY, JSON.stringify(map));
    }
    catch {
        /* localStorage plein/indisponible : on continue sans persistance */
    }
}
