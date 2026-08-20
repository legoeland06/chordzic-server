/**
 * Piano Live — portage du rendu de `rusty-chord/src/outils.rs` (app Yew,
 * module `ui`) en TypeScript pur.
 *
 * Le rendu original génère une liste de touches `<li class="…">` avec un
 * ordre graphique fixe par octave (12 entrées : `white e`, `black cs`, …)
 * et la note MIDI d'une touche vaut `35 + index + 12 × octave` (octave 0 =
 * C2). Le style des touches (dégradés, ombres, coins) vient de son
 * style.css.
 *
 * ⚠️ Alignement sur le clavier réel (feedback Eric 19/08) : le piano doit
 * couvrir l'étendue du Roland (88 touches : A0 → C8, MIDI 21 → 108) et non
 * pas C2 → B8 — sinon le dessin est amputé d'une octave à gauche et a une
 * excroissance inutile à droite. La plage est donc paramétrable
 * (pitchMin/pitchMax) avec pour défaut l'étendue piano standard.
 *
 * Cette logique est extraite ici pour être testable sans DOM ; le composant
 * React (LivePiano.tsx) ne fait que la mettre en forme.
 */
/**
 * Ordre EXACT de `ARRAY_OF_GRAPH_NOTES` de outils.rs, conservé à
 * l'identique. Malgré les noms de classes (e, cs, d, ds, c, b, as, a,
 * gs, g, fs, f — héritage du codepen d'origine), l'ordre correspond à un
 * clavier normal : C, C#, D, D#, E, F, F#, G, G#, A, A#, B — soit
 * exactement `pitch % 12` (0 = C … 11 = B).
 */
export const GRAPH_KEYS = [
    { cls: 'white e', isBlack: false, name: 'C' },
    { cls: 'black cs', isBlack: true, name: 'C#' },
    { cls: 'white d', isBlack: false, name: 'D' },
    { cls: 'black ds', isBlack: true, name: 'D#' },
    { cls: 'white c', isBlack: false, name: 'E' },
    { cls: 'white b', isBlack: false, name: 'F' },
    { cls: 'black as', isBlack: true, name: 'F#' },
    { cls: 'white a', isBlack: false, name: 'G' },
    { cls: 'black gs', isBlack: true, name: 'G#' },
    { cls: 'white g', isBlack: false, name: 'A' },
    { cls: 'black fs', isBlack: true, name: 'A#' },
    { cls: 'white f', isBlack: false, name: 'B' },
];
/**
 * Étendue par défaut : clavier piano 88 touches (A0 → C8, MIDI 21 → 108) —
 * celle des Roland « Digital Piano » (FP/RD…). Le dessin couvre alors
 * exactement les notes accessibles sur le clavier.
 */
export const LIVE_PIANO_MIN_PITCH = 21; // A0
export const LIVE_PIANO_MAX_PITCH = 108; // C8
/** Chromatic (0-11) d'un pitch, ou -1 hors de la plage [min, max]. */
export function chromaticOf(pitch) {
    const c = ((pitch % 12) + 12) % 12;
    return c;
}
/**
 * Construit les touches du piano sur la plage [pitchMin, pitchMax]
 * (défaut : A0 → C8 = 88 touches, l'étendue d'un Roland 88 notes).
 * Une touche noire suit sa blanche dans le flux (marge -1em du CSS).
 */
export function buildPianoKeys(pitchMin = LIVE_PIANO_MIN_PITCH, pitchMax = LIVE_PIANO_MAX_PITCH) {
    const keys = [];
    for (let p = pitchMin; p <= pitchMax; p++) {
        const def = GRAPH_KEYS[chromaticOf(p)];
        const octave = Math.floor(p / 12) - 1;
        keys.push({
            ...def,
            pitch: p,
            octave,
            noteName: `${def.name}${octave}`,
        });
    }
    return keys;
}
/**
 * Note MIDI → index dans GRAPH_KEYS (0..11), ou -1 si la note est hors de
 * la plage du piano.
 */
export function pitchToGraphIndex(pitch, pitchMin = LIVE_PIANO_MIN_PITCH, pitchMax = LIVE_PIANO_MAX_PITCH) {
    if (pitch < pitchMin || pitch > pitchMax)
        return -1;
    return chromaticOf(pitch);
}
/**
 * Largeur du piano en em (pour le fit scale).
 *
 * Chaque touche avance le flux de `width − |margin-left|` (la marge
 * négative tire la touche vers la gauche) :
 * - noire (2em, marge -1em)            → 1em
 * - blanche C/F (4em, sans marge)      → 4em
 * - blanche D/E/G/A/B (4em, marge -1em → 3em — règle `.a,.g,.f,.d,.c`)
 * La PREMIÈRE touche fait toujours sa largeur pleine (règle CSS
 * `li:first-child { margin-left: 0 }` — sinon elle déborderait du cadre).
 * Vérifié sur A0→C8 : 4+1+3 + 7×28 + 4 = 208em ; C1→C8 : 7×28+4 = 200em.
 */
export function pianoWidthEm(pitchMin = LIVE_PIANO_MIN_PITCH, pitchMax = LIVE_PIANO_MAX_PITCH) {
    let w = 0;
    for (let p = pitchMin; p <= pitchMax; p++) {
        const def = GRAPH_KEYS[chromaticOf(p)];
        if (p === pitchMin) {
            w += def.isBlack ? 2 : 4; // 1re touche : largeur pleine, sans marge
        }
        else if (def.isBlack) {
            w += 1;
        }
        else if (def.cls === 'white e' || def.cls === 'white b') {
            w += 4; // C et F : pas de marge
        }
        else {
            w += 3; // D, E, G, A, B : marge -1em
        }
    }
    return w;
}
/**
 * Ensemble des pitchs actifs bornés à la plage du piano.
 * (Les notes hors plage — pédales, percussions, CC… — n'illuminent rien.)
 */
export function activePitchSet(activePitches, pitchMin = LIVE_PIANO_MIN_PITCH, pitchMax = LIVE_PIANO_MAX_PITCH) {
    return new Set(activePitches.filter(p => Number.isInteger(p) && p >= pitchMin && p <= pitchMax));
}
/** Bornes de l'échelle (font-size en px) : lisibilité min, confort max. */
export const PIANO_SCALE_MIN = 3;
export const PIANO_SCALE_MAX = 14;
/**
 * Échelle (font-size en px) pour que le piano (largeur `totalEm` em,
 * cf. pianoWidthEm) tienne dans `containerWidth` pixels (+ 2px bordures).
 * Bornée par PIANO_SCALE_MIN/MAX (au-delà, le piano déborde légèrement —
 * l'overflow-x-auto du conteneur prend le relais).
 */
export function computePianoFontSize(containerWidth, totalEm) {
    const raw = (containerWidth - 2) / totalEm;
    return Math.min(Math.max(raw, PIANO_SCALE_MIN), PIANO_SCALE_MAX);
}
