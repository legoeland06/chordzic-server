/**
 * Garde de lecture — décide si un projet contient quelque chose à jouer.
 *
 * Un projet se compose de DEUX sources possibles :
 * - la grille Live (accords, input) ;
 * - les notes des piano rolls (mode Navig).
 *
 * La lecture est possible si AU MOINS une des deux contient des éléments —
 * c'est ce qui permet de lire en mode Navig un contenu créé uniquement en
 * notes, sans aucune grille Live (et vice-versa). L'alerte « rien à jouer »
 * ne doit retentir que si les DEUX sont vides.
 */
export function hasPlayableContent(chordsLength, notesLength) {
    return chordsLength > 0 || notesLength > 0;
}
/**
 * Vrai si le projet contient quelque chose à SAUVEGARDER (mode Live : grille
 * texte ; mode Navig : notes de piano roll — l'input peut être vide).
 */
export function hasSaveableContent(input, notesLength) {
    return input.trim() !== '' || notesLength > 0;
}
