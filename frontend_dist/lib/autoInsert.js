/**
 * Timer d'insertion automatique de l'accord détecté (mode Live).
 *
 * Logique pure, testable : quand le pianiste tient un accord identifié
 * pendant `delayMs` sans que les notes changent, l'accord est inséré
 * automatiquement dans la grille — pas besoin de cliquer « + Grille »
 * (les deux mains sont occupées à jouer).
 *
 * Règles :
 * - Un changement d'accord (même sans relâcher) redémarre le timer.
 * - Un accord déjà inséré automatiquement n'est PAS réinséré tant qu'il
 *   est tenu (pas de doublon) ; après une relâche complète, il peut être
 *   réinséré (l'utilisateur peut rejouer le même accord plus tard).
 * - Les notes brutes (accord non identifié) n'arment pas le timer.
 */
export const initialAutoInsertState = () => ({
    prevKey: null,
    firstSeenAt: null,
    lastKey: null,
});
/**
 * Décide si l'accord courant doit être inséré automatiquement.
 *
 * @param state  état du timer (persisté entre les ticks)
 * @param now    instant courant (ms)
 * @param delayMs délai d'insertion
 * @param key    clé de l'accord courant (null si aucune note)
 * @param insertable l'accord est identifié (insérable)
 * @returns le prochain état + le verdict d'insertion
 */
export function computeAutoInsert(state, now, delayMs, key, insertable) {
    // Pas d'accord armé (relâché ou notes brutes) : timer à zéro.
    // Après une relâche complète (key null), le même accord pourra être
    // réinséré (lastKey remis à null).
    if (!key || !insertable) {
        return {
            next: {
                prevKey: key,
                firstSeenAt: null,
                lastKey: key ? state.lastKey : null,
            },
            shouldInsert: false,
        };
    }
    // Déjà inséré automatiquement et toujours tenu → pas de doublon.
    if (key === state.lastKey) {
        return { next: { ...state, prevKey: key }, shouldInsert: false };
    }
    // Nouvel accord (différent du précédent) → le timer repart de zéro.
    if (key !== state.prevKey) {
        return {
            next: { prevKey: key, firstSeenAt: now, lastKey: state.lastKey },
            shouldInsert: false,
        };
    }
    // Même accord tenu : timer écoulé → insertion + armement anti-doublon.
    if (state.firstSeenAt !== null && now - state.firstSeenAt >= delayMs) {
        return {
            next: { prevKey: key, firstSeenAt: now, lastKey: key },
            shouldInsert: true,
        };
    }
    return { next: { ...state, prevKey: key }, shouldInsert: false };
}
