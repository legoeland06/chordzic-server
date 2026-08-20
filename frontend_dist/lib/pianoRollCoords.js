/**
 * Coordonnées du PianoRoll — helpers PURS (testables).
 *
 * Convention : la grille (beat 0) a son origine à x = 0, ALIGNÉE avec les
 * lanes compactes des autres pistes. Le clavier de piano est une COLONNE
 * FIXE à droite (hors de la zone scrollable) : aucun décalage de contenu,
 * aucun recouvrement — le repère reste visible en toutes circonstances.
 */
import { PIANO_KEYBOARD_WIDTH } from './pianoRollTypes';
/** Largeur de la colonne clavier (fixe, à droite de la zone d'édition). */
export { PIANO_KEYBOARD_WIDTH };
/** x écran d'un beat (pixels), scrollLeft inclus. */
export function xFromBeat(beat, ppb, scrollLeft) {
    return beat * ppb - scrollLeft;
}
/** Beat sous une position écran x (pixels). */
export function beatFromX(x, ppb, scrollLeft) {
    return (x + scrollLeft) / ppb;
}
