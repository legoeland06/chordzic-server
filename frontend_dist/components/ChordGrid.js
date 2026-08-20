import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * ChordGrid — affichage visuel de la grille d'accords.
 *
 * Chaque accord est affiché sur une ligne avec :
 * - Poignée de drag & drop (↕) pour réordonner
 * - Le chiffrage (ex: "Cm7") avec un code couleur
 * - La durée en temps
 * - Les notes qui composent l'accord
 * - Bouton de suppression
 *
 * L'accord en cours de lecture est surligné (highlighted).
 * Le drag & drop est désactivé pendant la lecture.
 */
import { memo } from 'react';
import { GripVertical } from 'lucide-react';
import { durationLabel, getChordColor, getNoteColor } from '../types/chord';
function ChordGrid({ chords, highlighted, playing, dragIdx, tempo, onClickChord, onDragStart, onDragOver, onDrop, onDragEnd, onDeleteChord, }) {
    if (chords.length === 0)
        return null;
    return (_jsxs("div", { className: "bg-gray-900 rounded-xl border border-gray-800 overflow-hidden", children: [_jsx("div", { className: "px-4 py-3 border-b border-gray-800", children: _jsxs("h2", { className: "text-sm font-bold text-blue-400", children: ["\uD83D\uDCCA Session \u00A0|\u00A0 ", tempo, " bpm \u00A0\u00B7\u00A0 ", chords.length, " accords", _jsx("span", { className: "text-[10px] text-gray-500 ml-3 font-normal", children: "\u2195 glisser pour r\u00E9ordonner" })] }) }), chords.map((c, idx) => (_jsx("div", { draggable: !playing, onDragStart: () => onDragStart(idx), onDragOver: onDragOver, onDrop: () => onDrop(idx), onDragEnd: onDragEnd, className: `px-3 py-3 border-b border-gray-800 last:border-0 transition-all duration-200 ${
                // Surbrillance de l'accord en cours
                highlighted === idx
                    ? 'bg-gray-700/60 ring-1 ring-blue-500/30'
                    : dragIdx === idx
                        ? 'opacity-40 bg-gray-800' // Élément déplacé → fantôme
                        : 'hover:bg-gray-800/50'} ${!playing ? 'cursor-grab active:cursor-grabbing' : ''}`, children: _jsxs("div", { className: "flex items-center gap-2", children: [_jsx("span", { className: "text-gray-600 shrink-0 select-none", children: _jsx(GripVertical, { className: "w-3.5 h-3.5" }) }), _jsxs("button", { onClick: () => c.chiffrage !== '_' && onClickChord(c), className: `w-28 shrink-0 text-left bg-transparent border-0 p-0 ${c.chiffrage === '_' ? 'cursor-default opacity-50' : 'cursor-pointer'}`, title: "Voir les d\u00E9tails", children: [_jsx("span", { className: "text-lg font-bold font-mono", style: { color: getChordColor(idx) }, children: c.chiffrage === '_' ? '—' : c.chiffrage }), _jsx("span", { className: "text-xs text-gray-500 ml-2", children: durationLabel(c.time) })] }), _jsx("div", { className: "flex flex-wrap gap-1.5", children: c.notes.map((note, ni) => (_jsx("span", { className: "px-2.5 py-1 rounded-md text-xs font-mono font-bold border", style: {
                                    color: getNoteColor(note),
                                    backgroundColor: 'rgba(40,40,40,0.8)',
                                    borderColor: 'rgba(60,60,60,0.8)',
                                }, children: note }, ni))) }), _jsx("button", { onClick: () => onDeleteChord(idx), className: "ml-auto text-gray-600 hover:text-red-400 transition-colors shrink-0", title: "Supprimer cet accord", children: "\u2715" })] }) }, idx)))] }));
}
export default memo(ChordGrid);
