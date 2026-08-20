/**
 * Mode de rendu WAV (mode Navig) : Interne (FluidSynth — rapide, silencieux,
 * toujours disponible) ou Externe (périphérique MIDI branché — Roland,
 * expander… — enregistré via sa sortie audio ; temps réel).
 */
/** Endpoint de rendu selon le moteur choisi. */
export function renderEndpoint(renderer) {
    return renderer === 'external' ? '/render-external' : '/render-wav';
}
/** Configuration du clic selon le moteur :
 * - Externe : le clic est TOUJOURS mixé après capture par le serveur
 *   (jamais de mode séparé — le serveur enregistre le périphérique).
 * - Interne : comportement historique (séparé si sortie dédiée choisie). */
export function clickModeForRenderer(renderer, cfg) {
    if (renderer === 'external') {
        return { click_in_render: !!cfg.in_render };
    }
    if (cfg.out_device && !cfg.in_render) {
        return { click_separate: true };
    }
    return { click_in_render: !!cfg.in_render };
}
/** Libellé français du moteur (pour l'UI). */
export function rendererLabel(renderer) {
    return renderer === 'external' ? 'Externe' : 'Interne';
}
