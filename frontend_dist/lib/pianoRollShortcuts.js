/** Vrai si la cible est un champ de saisie / bouton (saisie utilisateur). */
export function isTypingTarget(target) {
    const t = target;
    if (!t || typeof t.tagName !== 'string')
        return false;
    const tag = t.tagName;
    return tag === 'BUTTON' || tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA';
}
/** Résout la touche → action (null si aucune correspondance). */
export function pianoRollShortcut(e) {
    if (isTypingTarget(e.target))
        return null;
    const mod = !!(e.ctrl || e.meta);
    const k = String(e.key).toLowerCase();
    if (mod) {
        if (k === 'g')
            return 'group';
        if (k === 'u')
            return 'ungroup';
        if (k === ' ')
            return 'play-audio'; // Ctrl+Espace = lecture audio globale
        return null;
    }
    if (e.shift && k === ' ')
        return 'play-midi'; // Shift+Espace = lecture MIDI
    switch (k) {
        case 'e': return 'tool-edit';
        case 'v': return 'tool-select';
        case 'q': return 'quantize';
        case '*': return 'rec';
        case '0': return 'go-start';
        case '1': return 'go-loc-l';
        case '2': return 'go-loc-r';
        case 'o': return 'zoom-selection';
        default: return null;
    }
}
