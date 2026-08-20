/**
 * Types et fonctions utilitaires pour la manipulation des accords.
 *
 * Point central de la logique harmonique : parse les notations textuelles
 * (ex: "4:Cm7", "2:Fmaj7/G") en données structurées ChordData avec :
 * - Notes MIDI (avec octaves)
 * - Qualités d'accords (70+ types, de la triade aux 13èmes altérées)
 * - Intervalles
 *
 * Le format d'entrée : `<durée en temps>:<Fondamentale><Qualité>[/<Basse alternative>]`
 */
// ─── Mapping notes → MIDI ──────────────────────────────────────────────
/** Index chromatique de chaque note depuis C (0) jusqu'à B (11).
 *  Les bémols sont redirigés vers leur équivalent dièse. */
export const NOTE_TO_MIDI = {
    'C': 0, 'C#': 1, 'Db': 1,
    'D': 2, 'D#': 3, 'Eb': 3,
    'E': 4, 'F': 5, 'F#': 6,
    'Gb': 6, 'G': 7, 'G#': 8,
    'Ab': 8, 'A': 9, 'A#': 10,
    'Bb': 10, 'B': 11,
};
/** Tableau des noms de notes dans l'ordre chromatique. */
export const NOTE_NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
// ─── Qualités d'accords — intervalles ──────────────────────────────────
/**
 * Toutes les qualités d'accords (70+) venues du moteur Java original.
 * Chaque entrée est un tableau d'intervalles en demi-tons depuis la
 * fondamentale. Les intervalles ≥ 12 ajoutent une octave (ex: 9 = 14,
 * 13 = 21), ce qui encode directement la hauteur des notes dans l'octave.
 */
export const QUALITY_INTERVALS = {
    // ── Triades de base ──
    '': [0, 4, 7], // Maj (par défaut)
    'M': [0, 4, 7], // Maj
    'maj': [0, 4, 7],
    'min': [0, 3, 7],
    'm': [0, 3, 7], // mineur
    '-': [0, 3, 7],
    'dim': [0, 3, 6], // diminuée
    '(b5)': [0, 4, 6], // Maj b5
    'aug': [0, 4, 8], // augmentée
    '+': [0, 4, 8],
    'sus2': [0, 2, 7],
    'sus4': [0, 5, 7],
    'sus': [0, 5, 7],
    '5': [0, 7], // quinte (power chord)
    // ── Accords spéciaux (sans tierce) ──
    'no5': [0, 4],
    'omit5': [0, 4],
    'm(no5)': [0, 3],
    'm(omit5)': [0, 3],
    // ── Sixte ──
    '6': [0, 4, 7, 9],
    'm6': [0, 3, 7, 9],
    'dim6': [0, 3, 6, 8],
    // ── Septième ──
    '7': [0, 4, 7, 10],
    '7b5': [0, 4, 6, 10],
    '7-5': [0, 4, 6, 10],
    '7#5': [0, 4, 8, 10],
    '7+5': [0, 4, 8, 10],
    '7sus4': [0, 5, 7, 10],
    'm7': [0, 3, 7, 10],
    'm7b5': [0, 3, 6, 10],
    'm7-5': [0, 3, 6, 10],
    'm7#5': [0, 3, 8, 10],
    'm7+5': [0, 3, 8, 10],
    'dim7': [0, 3, 6, 9],
    '7alt': [0, 3, 6, 9], // alt = dim7
    // ── Septième majeure ──
    'M7': [0, 4, 7, 11],
    'maj7': [0, 4, 7, 11],
    'M7#5': [0, 4, 8, 11],
    'M7+5': [0, 4, 8, 11],
    'mM7': [0, 3, 7, 11],
    // ── Add (notes ajoutées sans septième) ──
    'add4': [0, 4, 5, 7],
    'Madd4': [0, 4, 5, 7],
    'madd4': [0, 3, 5, 7],
    'add9': [0, 4, 7, 14],
    'Madd9': [0, 4, 7, 14],
    'madd9': [0, 3, 7, 14],
    'add11': [0, 4, 7, 17],
    '2': [0, 4, 7, 14], // add9
    '4': [0, 4, 7, 17], // add11
    // ── Sus avec ajouts ──
    'sus4add9': [0, 5, 7, 14],
    'sus4add2': [0, 2, 5, 7],
    '9sus4': [0, 5, 7, 10, 14],
    // ── Neuvième ──
    '9': [0, 4, 7, 10, 14],
    'm9': [0, 3, 7, 10, 14],
    'M9': [0, 4, 7, 11, 14],
    'maj9': [0, 4, 7, 11, 14],
    '9b5': [0, 4, 6, 10, 14],
    '9-5': [0, 4, 6, 10, 14],
    '9#5': [0, 4, 8, 10, 14],
    '9+5': [0, 4, 8, 10, 14],
    '7b9': [0, 4, 7, 10, 13],
    '7-9': [0, 4, 7, 10, 13],
    '7#9': [0, 4, 7, 10, 15],
    '7+9': [0, 4, 7, 10, 15],
    '7b9b5': [0, 4, 6, 10, 13],
    '7b9#5': [0, 4, 8, 10, 13],
    '7#9b5': [0, 4, 6, 10, 15],
    '7#9#5': [0, 4, 8, 10, 15],
    'm7b9b5': [0, 3, 6, 10, 13],
    // ── Onzième ──
    '11': [0, 7, 10, 14, 17],
    'm11': [0, 3, 7, 17],
    '7#11': [0, 4, 7, 10, 18],
    '7+11': [0, 4, 7, 10, 18],
    'M7#11': [0, 4, 7, 11, 18],
    'M7+11': [0, 4, 7, 11, 18],
    '7b9#9': [0, 4, 7, 10, 13, 15],
    '7b9#11': [0, 4, 7, 10, 13, 18],
    '7#9#11': [0, 4, 7, 10, 15, 18],
    'm7add11': [0, 3, 7, 10, 17],
    'M7add11': [0, 4, 7, 11, 17],
    'mM7add11': [0, 3, 7, 11, 17],
    // ── Treizième ──
    '13': [0, 4, 7, 10, 14, 21],
    '13b9': [0, 4, 7, 10, 13, 21],
    '13-9': [0, 4, 7, 10, 13, 21],
    '13#9': [0, 4, 7, 10, 15, 21],
    '13+9': [0, 4, 7, 10, 15, 21],
    '13#11': [0, 4, 7, 10, 18, 21],
    '13+11': [0, 4, 7, 10, 18, 21],
    '7b13': [0, 4, 7, 10, 20],
    '7-13': [0, 4, 7, 10, 20],
    '7b9b13': [0, 4, 7, 10, 13, 17, 20],
    // ── Spéciales ──
    'm69': [0, 3, 7, 9, 14],
    '69': [0, 4, 7, 9, 14],
    'Mb5': [0, 4, 6],
    'ø': [0, 3, 6, 10], // demi-diminué = m7b5
    '°': [0, 3, 6], // diminué
    '9#11': [0, 4, 7, 10, 14, 18],
    '9+11': [0, 4, 7, 10, 14, 18],
    'M7add13': [0, 4, 7, 9, 11, 14],
    'm13': [0, 3, 7, 10, 14, 21],
    'dim/M7': [0, 3, 6, 11],
    'mb5': [0, 4, 6],
    'm7b9': [0, 3, 7, 10, 13],
};
// ─── Parseurs ───────────────────────────────────────────────────────────
/**
 * Parse une chaîne d'accord au format "4:Cm7" ou "4:Cmaj7/G".
 *
 * Étapes :
 * 1. Extraire la durée (temps) avant les ":"
 * 2. Si le reste est "_", c'est un silence → notes vides
 * 3. Extraire la basse alternative après "/" si présente
 * 4. Extraire la fondamentale (note) et sa qualité
 * 5. Résoudre les intervalles de la qualité → notes MIDI
 * 6. Dédupliquer et retourner ChordData
 */
export function parseChord(input) {
    // Séparer durée et reste
    const parts = input.split(':');
    const time = parseInt(parts[0]) || 4;
    const rest = parts[1] || parts[0];
    // Silence
    if (rest.trim() === '_') {
        return {
            time, name: '_', quality: '', bass: '',
            chiffrage: '_', notes: [], midiValues: [],
        };
    }
    // Extraire la basse alternative (après "/")
    let chordStr = rest;
    let bass = '';
    if (rest.includes('/')) {
        const split = rest.split('/');
        chordStr = split[0];
        bass = split[1];
    }
    // Extraire la fondamentale (1ère note avec altération optionnelle)
    const noteMatch = chordStr.match(/^([A-G][#b]?)(.*)/);
    if (!noteMatch)
        throw new Error(`Format invalide: ${input}`);
    const name = noteMatch[1]; // Ex: "C", "F#", "Bb"
    const quality = noteMatch[2] || 'M'; // Ex: "m7", "maj7", "dim" (défaut: Majeure)
    const bassNote = bass || name;
    // Résoudre les intervalles
    const rootVal = NOTE_TO_MIDI[name] ?? 0;
    const intervals = resolveQuality(quality);
    const rawValues = [];
    for (const i of intervals) {
        const v = rootVal + i;
        if (!rawValues.includes(v)) {
            rawValues.push(v);
        }
    }
    // Noms des notes (en 0-11 pour l'affichage, mais on garde l'octave pour le MIDI)
    const notes = rawValues.map((v) => NOTE_NAMES[v % 12]);
    // ── Chiffrage d'affichage propre ──
    // 1. Pas de « / » tant que l'utilisateur ne spécifie pas une basse
    //    alternative (ex. "1:Am7/D") — un simple "2:G7" s'affiche "G7",
    //    plus jamais "G7/".
    // 2. Les triades majeures s'affichent sans suffixe : "C", "C#", "D"…
    //    (plus de "CM" / "C#M"). Les extensions (M7, M9…) gardent leur
    //    suffixe : "CM7" reste "CM7".
    const ql = quality.toLowerCase();
    const isMajorTriad = quality === '' || quality === 'M' || ql === 'maj';
    const displayQuality = isMajorTriad ? '' : quality;
    const hasBass = bass !== '' && bass !== name;
    const chiffrage = `${name}${displayQuality}${hasBass ? '/' + bass : ''}`;
    return {
        time, name, quality, bass: bassNote, chiffrage, notes, midiValues: rawValues,
    };
}
/**
 * Résout une qualité textuelle en tableau d'intervalles.
 * Gère les alias (min→m, maj→M, les parenthèses, etc.).
 * Fallback sur Majeure si la qualité est inconnue.
 */
function resolveQuality(q) {
    const cleaned = q.trim().replace(/[()]/g, '');
    // Recherche exacte
    if (QUALITY_INTERVALS[cleaned])
        return QUALITY_INTERVALS[cleaned];
    // Essayer minuscule / majuscule
    const lowered = cleaned.toLowerCase();
    const uppered = cleaned.toUpperCase();
    if (QUALITY_INTERVALS[lowered])
        return QUALITY_INTERVALS[lowered];
    if (QUALITY_INTERVALS[uppered])
        return QUALITY_INTERVALS[uppered];
    // Fallback Majeure
    console.warn(`Qualité inconnue: "${q}", fallback Maj`);
    return QUALITY_INTERVALS['M'];
}
/**
 * Parse une grille complète (espacement des accords).
 * Format : "4:Cm7 4:F7 2:G7 4:C".
 *
 * La notation de répétition « xN » est supportée : "2:Cm7x3" est
 * équivalent à "2:Cm7 2:Cm7 2:Cm7" (N ≥ 1 ; x0 est clampé à 1).
 */
export function parseGrille(input, tempo = 120) {
    const parts = input.trim().split(/\s+/);
    const chords = [];
    for (const p of parts) {
        const { base, repeat } = parseRepeat(p);
        const c = parseChord(base);
        for (let i = 0; i < repeat; i++) {
            // Copie profonde : chaque occurrence est indépendante (notes et
            // midiValues ne sont jamais mutés, mais autant être propre).
            chords.push({ ...c, notes: [...c.notes], midiValues: [...c.midiValues] });
        }
    }
    return { titre: 'Session', tempo, chords };
}
/**
 * Extrait le facteur de répétition « xN » d'un token de grille.
 * Ex. "2:Cm7x3" → { base: "2:Cm7", repeat: 3 }. Sans « xN » : repeat 1.
 * N est clampé à ≥ 1 (x0 = 1 fois, jamais d'accord qui disparaît).
 */
export function parseRepeat(token) {
    const m = token.match(/^(.*)x(\d+)$/);
    if (m) {
        return { base: m[1], repeat: Math.max(1, parseInt(m[2], 10)) };
    }
    return { base: token, repeat: 1 };
}
/** Figures rythmiques pour la notation de durée (N = diviseur de la mesure).
 * Dans une mesure de 4 temps : 1 = ronde (4 t), 2 = blanche (2 t),
 * 4 = noire (1 t), 6 = triolet de noire (2/3 t), 8 = croche (1/2 t),
 * 12 = triolet de croche (1/3 t), 16 = double croche, 24 = sextolet,
 * 32 = triple croche, 64 = quadruple croche.
 * Les diviseurs sans figure standard (3, 5, 7…) → « N par mesure ». */
export function durationLabel(time) {
    const figures = {
        1: 'ronde', 2: 'blanche', 4: 'noire', 8: 'croche',
        16: 'double croche', 32: 'triple croche', 64: 'quadruple croche',
        3: '3 par mesure', 6: 'triolet de noire', 12: 'triolet de croche', 24: 'sextolet de croche',
    };
    return figures[time] ?? `${time} par mesure`;
}
// ─── Couleurs ───────────────────────────────────────────────────────────
/** Palette cyclique pour colorer les accords. */
export function getChordColor(idx) {
    const colors = [
        '#3b82f6', '#ef4444', '#22c55e', '#eab308', '#f97316',
        '#a855f7', '#ec4899', '#14b8a6', '#8b5cf6', '#f43f5e',
    ];
    return colors[idx % colors.length];
}
/** Couleur par note (pour l'affichage des notes individuelles). */
export function getNoteColor(note) {
    if (note.includes('#') || note.includes('b'))
        return '#60a5fa';
    const colors = {
        'C': '#ffffff', 'D': '#fbbf24', 'E': '#67e8f9',
        'F': '#86efac', 'G': '#fb923c', 'A': '#fca5a5', 'B': '#c4b5fd',
    };
    return colors[note] || '#d1d5db';
}
