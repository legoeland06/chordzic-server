import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * ChordNowModal — affichage de l'accord en cours de lecture (mode Live).
 *
 * Une « modal » circulaire, translucide (fond sombre + flou d'arrière-plan),
 * centrée sur l'écran, avec une TRÈS grosse police : l'utilisateur voit
 * d'un coup d'œil l'accord joué par la séquence (et le suivant, en petit).
 * `pointer-events-none` : ne bloque ni les clics ni la lecture.
 *
 * Remplaçant de l'ancien affichage du TrackPanel (perdu à sa suppression),
 * avec un design entièrement revu.
 */
import { memo } from 'react';
import { getChordColor } from '../types/chord';
function ChordNowModal({ chords, highlighted, playing }) {
    const current = playing && highlighted >= 0 ? chords[highlighted] : undefined;
    const next = playing && highlighted + 1 < chords.length ? chords[highlighted + 1] : undefined;
    if (!current)
        return null;
    return (_jsx("div", { className: "fixed inset-0 z-40 flex items-center justify-center pointer-events-none", children: _jsxs("div", { className: "w-72 h-72 sm:w-96 sm:h-96 rounded-full bg-gray-950/50 backdrop-blur-sm border border-gray-700/40 shadow-[0_0_80px_rgba(0,0,0,0.6)] flex flex-col items-center justify-center gap-1.5 px-6", children: [_jsx("div", { className: "text-[10px] uppercase tracking-[0.25em] text-gray-500 select-none", children: "En lecture" }), _jsx("div", { className: "text-7xl sm:text-8xl font-bold font-mono leading-none select-none", style: { color: getChordColor(highlighted) }, title: "Accord jou\u00E9 par la s\u00E9quence", children: current.chiffrage === '_' ? '\u2014' : current.chiffrage }), next && (_jsx("div", { className: "text-xl sm:text-2xl font-mono opacity-40 select-none", style: { color: getChordColor(highlighted + 1) }, title: "Accord suivant", children: next.chiffrage === '_' ? '\u2014' : next.chiffrage }))] }) }));
}
export default memo(ChordNowModal);
