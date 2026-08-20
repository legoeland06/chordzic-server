import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
// Mapping index chromatique → nom de note
const NOTE_NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
// Indices des touches blanches dans les 12 demi-tons
const WHITE_KEYS = [0, 2, 4, 5, 7, 9, 11];
/**
 * Mini clavier de piano, 2 octaves (C3 à B4 par défaut).
 *
 * Layout :
 * - Touches blanches : côte à côte, largeur égale
 * - Touches noires : positionnées par-dessus, décalées selon leur position
 *   dans l'octave (positions relatives aux touches blanches)
 *
 * La hauteur totale est fixe (84px) pour s'intégrer dans le modal de détail.
 */
export default function PianoKeyboard({ activeNotes, highlightedNotes, octaves = 2, startOctave = 3, }) {
    // Construire un Set pour lookup rapide (évite de re-parcourir le tableau)
    const activeSet = new Set(activeNotes ?? []);
    const legacySet = new Set(highlightedNotes ?? []);
    // Tableaux des touches avec leurs propriétés de rendu
    const whiteKeys = [];
    const blackKeys = [];
    // Générer toutes les touches sur les octaves demandées
    for (let o = startOctave; o < startOctave + octaves; o++) {
        for (let i = 0; i < 12; i++) {
            const noteName = NOTE_NAMES[i];
            const fullName = `${noteName}${o}`;
            let isHighlighted;
            if (activeSet.size > 0) {
                // Mode notes précises avec octave
                isHighlighted = activeSet.has(fullName);
            }
            else {
                // Mode legacy : toutes les octaves
                isHighlighted = legacySet.has(noteName);
            }
            if (WHITE_KEYS.includes(i)) {
                whiteKeys.push({ note: noteName, octave: o, isHighlighted });
            }
            else {
                blackKeys.push({ note: noteName, octave: o, isHighlighted });
            }
        }
    }
    // Largeur de chaque touche blanche en pourcentage
    const whiteKeyWidth = 100 / whiteKeys.length;
    return (_jsxs("div", { className: "relative w-full", style: { height: 84 }, children: [_jsx("div", { className: "absolute inset-0 flex", children: whiteKeys.map((k) => (_jsx("div", { draggable: false, className: `
              border-r border-gray-700 last:border-r-0
              flex items-end justify-center pb-1
              text-[8px] font-mono select-none
              transition-colors duration-150 cursor-default
              ${k.isHighlighted
                        ? 'bg-blue-500 text-white font-bold shadow-inner shadow-blue-300/40 z-10'
                        : 'bg-white text-gray-400'}
            `, style: { width: `${whiteKeyWidth}%`, height: 84 }, title: `${k.note}${k.octave}`, children: k.note }, `w-${k.note}${k.octave}`))) }), _jsx("div", { className: "absolute inset-0", style: { height: 52, pointerEvents: 'none' }, children: blackKeys.map((k, i) => {
                    // Positions relatives des noires dans l'octave (index des blanches)
                    // Do# = entre Do et Ré, Ré# = entre Ré et Mi, etc.
                    const octaveOffset = Math.floor(i / 5) * 7;
                    const blackPositions = [0.6, 1.6, 3.6, 4.6, 5.6];
                    const posInOctave = i % 5;
                    const whiteIdx = octaveOffset + Math.floor(blackPositions[posInOctave]);
                    return (_jsx("div", { className: `
                absolute bottom-0 rounded-b-[3px]
                transition-colors duration-150
                ${k.isHighlighted
                            ? 'bg-blue-700 shadow-inner shadow-blue-400/30 z-20'
                            : 'bg-gray-900 z-10'}
              `, style: {
                            left: `calc(${whiteIdx * whiteKeyWidth}% + ${whiteKeyWidth * 0.55}%)`,
                            width: `${whiteKeyWidth * 0.75}%`,
                            height: 52,
                        }, title: `${k.note}${k.octave}` }, `b-${k.note}${k.octave}`));
                }) }), _jsxs("div", { className: "absolute -bottom-3.5 left-0 right-0 text-center text-[7px] text-gray-600 select-none", children: ["C", startOctave, " \u2014 B", startOctave + octaves - 1] })] }));
}
