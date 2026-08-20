/**
 * Autocomplete — helpers d'autocomplétion partagés entre ChordApp,
 * ChordInput et ChordDetailModal.
 *
 * Évite la triple duplication de cette logique dans 3 fichiers.
 */
import { QUALITY_INTERVALS } from '../types/chord';
// Regex pour extraire fondamentale + qualité d'un nom d'accord
const NOTE_PATTERN = /^([A-G][#b]?)(.*)/;
// ─── Qualités triées (pour le matching par préfixe) ─────────────────────
/** Liste triée des noms de qualité disponibles (des plus courts aux plus longs). */
export const QUALITY_NAMES = Object.keys(QUALITY_INTERVALS)
    .filter(k => k && k !== 'M' && k !== '')
    .sort((a, b) => a.length - b.length || a.localeCompare(b));
// ─── Helpers ────────────────────────────────────────────────────────────
/**
 * Récupère le token (mot) en cours d'édition à la position du curseur.
 * Les espaces sont les séparateurs de mots.
 */
export function getCurrentToken(text, cursor) {
    let start = cursor;
    while (start > 0 && text[start - 1] !== ' ')
        start--;
    let end = cursor;
    while (end < text.length && text[end] !== ' ')
        end++;
    return { start, end, token: text.slice(start, end) };
}
/**
 * Calcule les suggestions d'autocomplétion pour le token courant.
 *
 * 3 cas :
 * 1. Token avec ":" (ex: "4:Cm") → complète la qualité
 * 2. Token sans ":" (ex: "Cm") → idem, sans le préfixe de temps
 * 3. Token finissant par ":" (ex: "4:") → propose le dernier accord tapé
 */
export function getSuggestions(token, lastChordChiffrage) {
    if (!token || token === ' ')
        return [];
    const trimmed = token.trim();
    // Cas 1 : "4:Cm" → proposer les complétions de qualité
    if (trimmed.includes(':')) {
        const colonIdx = trimmed.indexOf(':');
        const timePart = trimmed.slice(0, colonIdx + 1);
        const rest = trimmed.slice(colonIdx + 1);
        const noteMatch = rest.match(NOTE_PATTERN);
        if (noteMatch) {
            const noteName = noteMatch[1];
            const partialQuality = noteMatch[2].toLowerCase();
            if (partialQuality !== rest.toLowerCase()) {
                const results = [];
                for (const q of QUALITY_NAMES) {
                    if (q.toLowerCase().startsWith(partialQuality)) {
                        results.push(timePart + noteName + q);
                    }
                }
                return results.slice(0, 12);
            }
        }
    }
    // Cas 2 : "Cm" (sans time) → proposer la qualité
    const noteMatch = trimmed.match(NOTE_PATTERN);
    if (noteMatch) {
        const noteName = noteMatch[1];
        const partialQuality = noteMatch[2].toLowerCase();
        const results = [];
        for (const q of QUALITY_NAMES) {
            if (q.toLowerCase().startsWith(partialQuality)) {
                results.push(noteName + q);
            }
        }
        return results.slice(0, 12);
    }
    // Cas 3 : "4:" → proposer le dernier accord tapé
    if (/^\d+:$/.test(trimmed) && lastChordChiffrage) {
        return [trimmed + lastChordChiffrage];
    }
    return [];
}
/**
 * Remplace un token dans la chaîne (entre start et end) par le replacement.
 */
export function replaceToken(text, start, end, replacement) {
    return text.slice(0, start) + replacement + text.slice(end);
}
