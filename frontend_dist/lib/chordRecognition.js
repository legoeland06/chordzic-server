/**
 * Reconnaissance d'accords — mode Live.
 *
 * Le serveur relaie les notes tenues sur le clavier du pianiste
 * (route GET /live-input) ; ce module les compare à l'harmonie intégrée
 * (QUALITY_INTERVALS) pour identifier l'accord plaqué.
 *
 * Règles :
 * - 1 note tenue        → affichage de la note seule (pas d'accord)
 * - 2 notes tenues      → match STRICT (exact uniquement)
 * - ≥ 3 notes tenues    → match exact d'abord, puis inclusion (l'accord
 *                         connu est un sous-ensemble des notes jouées)
 * - Basse réelle (note la plus basse) pour départager les accords relatifs
 *   (ex. C6 vs Am7 : C en basse → C6, A en basse → Am7)
 * - Aucun accord connu  → notes brutes affichées (le clavier est capté)
 *
 * Convention d'harmonie (demandée par Eric) :
 * - La note la plus grave, quand elle est à ≥ 1 octave (BASS_GAP) de la
 *   suivante, est une BASSE IMPOSÉE (choix délibéré du pianiste), jamais
 *   une note de l'accord (quinte, sixte, septième, neuvième…).
 * - Si cette basse ≠ fondamentale de l'accord reconnu → notation « Accord/Basse »
 *   (ex. C/E, C/G, Am7/D) ; si elle est la fondamentale → accord seul (C).
 */
import { NOTE_NAMES, QUALITY_INTERVALS, parseChord } from '../types/chord';
/** Écart minimal (demi-tons) entre la note la plus grave et la suivante
 * pour la considérer comme une basse imposée (main gauche) et non comme
 * une note de l'accord. Une octave est le seuil musical standard. */
export const BASS_GAP = 12;
/** Noms canoniques préférés, par ordre de priorité (les alias d'un même
 * accord sont fusionnés ; le premier nom préféré rencontré gagne). */
const PREFERRED_QUALITIES = [
    '', 'm', '7', 'M7', 'm7', 'dim', '(b5)', 'aug', 'sus4', 'sus2',
    '5', 'no5', 'm(no5)', '6', 'm6', 'dim6', '7b5', '7#5', '7sus4',
    'm7b5', 'm7#5', 'dim7', 'M7#5', 'mM7', 'add4', 'madd4', 'add9',
    'madd9', '11', '9', 'm9', 'M9', '9b5', '9#5', '7b9', '7#9', '13',
    'm11', '7#11', 'M7#11', 'm7add11', 'M7add11', 'mM7add11', '69',
    'm69', '7b9b5', '7b9#5', '7#9b5', '7#9#5', 'm7b9b5', '7b9#9',
    '7b9#11', '7#9#11', 'M7add13', 'm13', '13b9', '13#9', '13#11',
    '7b13', '7b9b13', '9#11', 'm7b9', 'Mb5', 'mb5', 'dim/M7', 'sus4add9',
    'sus4add2', '9sus4', 'm7b9b5', '7alt', 'ø', '°', 'Madd9', 'Madd4',
    '2', '4', 'madd9', 'M7+5', 'M7#5', '7+5', '7-5', 'm7+5', 'm7-5',
    '9+5', '9-5', '13+9', '13-9', '13+11', '9+11', '7+9', '7-9',
];
/** Classes de hauteur d'une liste d'intervalles (mod 12, triées, uniques). */
function classesOf(intervals) {
    return [...new Set(intervals.map(i => ((i % 12) + 12) % 12))].sort((a, b) => a - b);
}
/** Table des qualités : un seul nom par ensemble de classes (le préféré). */
function buildQualityTable() {
    const seen = new Set();
    const out = [];
    const push = (name) => {
        const iv = QUALITY_INTERVALS[name];
        if (!iv)
            return;
        const cls = classesOf(iv);
        const k = cls.join(',');
        if (seen.has(k))
            return;
        seen.add(k);
        out.push({ name, classes: cls });
    };
    for (const q of PREFERRED_QUALITIES)
        push(q);
    for (const q of Object.keys(QUALITY_INTERVALS))
        push(q);
    return out;
}
const QUALITY_TABLE = buildQualityTable();
function sameSet(a, b) {
    return a.length === b.length && a.every((v, i) => v === b[i]);
}
export function recognizeChord(pitches) {
    const valid = pitches.filter(p => Number.isInteger(p) && p >= 0 && p <= 127);
    if (valid.length === 0)
        return null;
    // ── Basse imposée (convention d'harmonie) ──
    // La note la plus grave, à ≥ 1 octave de la suivante, est un choix
    // délibéré du pianiste (main gauche) : elle n'appartient PAS à l'accord
    // à reconnaître. Si elle diffère de la fondamentale → « Accord/Basse ».
    const sorted = [...valid].sort((a, b) => a - b);
    const hasBass = sorted.length >= 2 && sorted[1] - sorted[0] >= BASS_GAP;
    const bassPitch = hasBass ? sorted[0] : null;
    const chordPitches = hasBass ? valid.filter(p => p !== bassPitch) : valid;
    const r = recognizeOn(chordPitches);
    if (!r)
        return null;
    // Classes complètes (basse incluse) pour l'information.
    const allClasses = [...new Set(valid.map(p => p % 12))].sort((a, b) => a - b);
    if (bassPitch !== null && r.insertable) {
        const bassClass = bassPitch % 12;
        if (bassClass !== r.root) {
            const label = parseChord(`4:${NOTE_NAMES[r.root]}${r.quality}/${NOTE_NAMES[bassClass]}`).chiffrage;
            return { ...r, label, classes: allClasses, bass: NOTE_NAMES[bassClass] };
        }
    }
    return { ...r, classes: allClasses, bass: null };
}
/** Reconnaît un accord sur un ensemble de pitchs SANS basse détachée.
 * (Logique principale : note seule, strict à 2 notes, tolérance ≥ 3,
 * basse relative pour départager C6 vs Am7.) */
function recognizeOn(pitches) {
    const valid = pitches.filter(p => Number.isInteger(p) && p >= 0 && p <= 127);
    if (valid.length === 0)
        return null;
    const classes = [...new Set(valid.map(p => p % 12))].sort((a, b) => a - b);
    const lowestClass = Math.min(...valid) % 12;
    // Une seule note tenue → la note, pas un accord.
    if (classes.length === 1) {
        return {
            root: classes[0], quality: '', label: NOTE_NAMES[classes[0]],
            classes, exact: true, noteOnly: true, insertable: true, bass: null,
        };
    }
    // Tolérance (inclusion) uniquement à partir de 3 notes plaquées.
    const allowInclusion = classes.length >= 3;
    let best = null;
    for (let root = 0; root < 12; root++) {
        for (const { name, classes: qc } of QUALITY_TABLE) {
            const expected = qc.map(c => (root + c) % 12).sort((a, b) => a - b);
            let score = sameSet(expected, classes) ? 1000 : 0;
            if (score === 0 && allowInclusion && expected.every(c => classes.includes(c))) {
                const foreign = classes.filter(c => !expected.includes(c)).length;
                score = 500 + expected.length * 10 - foreign;
            }
            if (score > 0) {
                // Bonus basse réelle : la fondamentale la plus probable est celle
                // qui correspond à la note la plus basse jouée (C6 vs Am7…).
                if (root === lowestClass)
                    score += 100;
                if (best === null || score > best.score) {
                    best = { root, q: name, score };
                }
            }
        }
    }
    if (!best) {
        // Aucun accord connu : notes brutes (le clavier est bien capté).
        return {
            root: classes[0], quality: '', label: classes.map(c => NOTE_NAMES[c]).join('·'),
            classes, exact: false, noteOnly: false, insertable: false, bass: null,
        };
    }
    // Chiffrage propre via l'harmonie intégrée (règles d'affichage : pas de
    // « M » sur les triades majeures, pas de « / » sans basse alternative).
    const label = parseChord(`4:${NOTE_NAMES[best.root]}${best.q}`).chiffrage;
    return {
        root: best.root, quality: best.q, label, classes,
        exact: best.score >= 1000, noteOnly: false, insertable: true, bass: null,
    };
}
