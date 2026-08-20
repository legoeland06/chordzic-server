import { jsx as _jsx, Fragment as _Fragment, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * ChordDetailModal — modal d'information détaillée sur un accord.
 *
 * Affiche : navigation ← →, chiffrage éditable, propriétés, piano,
 * notes avec intervalles, valeurs MIDI, play/stop.
 *
 * L'autocomplétion utilise le module partagé lib/autocomplete.ts.
 */
import { useState, useRef, useEffect } from 'react';
import PianoKeyboard from './PianoKeyboard';
import { NOTE_TO_MIDI, durationLabel, getNoteColor } from '../types/chord';
import { getSuggestions as getAutocompleteSuggestions } from '../lib/autocomplete';
import { notesWithOctave } from '../lib/chordUtils';
/** Noms des intervalles (pour l'affichage des notes). */
const INTERVAL_NAMES = [
    'P1', 'm2', 'M2', 'm3', 'M3', 'P4', 'b5',
    'P5', 'm6', 'M6', 'm7', 'M7',
];
export default function ChordDetailModal({ chord, chordIdx, chordsCount, playing, onClose, onTogglePlay, onPrev, onNext, onUpdateChord, }) {
    const [editText, setEditText] = useState('');
    const [editing, setEditing] = useState(false);
    const [suggestions, setSuggestions] = useState([]);
    const [suggestIdx, setSuggestIdx] = useState(0);
    const inputRef = useRef(null);
    // Met à jour le texte local quand l'accord change (hors édition)
    useEffect(() => {
        if (chord && !editing) {
            setEditText(`${chord.time}:${chord.chiffrage}`);
        }
    }, [chord, editing]);
    /** Valide l'édition et envoie la modification au parent. */
    const commitEdit = () => {
        if (!editText.trim() || chordIdx < 0)
            return;
        onUpdateChord(chordIdx, editText.trim());
        setEditing(false);
        setSuggestions([]);
    };
    /** Applique une suggestion (remplace le texte + place le curseur). */
    const applySuggestion = (suggestion) => {
        setEditText(suggestion);
        setSuggestions([]);
        setSuggestIdx(0);
        inputRef.current?.focus();
        requestAnimationFrame(() => {
            if (inputRef.current) {
                inputRef.current.setSelectionRange(suggestion.length, suggestion.length);
            }
        });
    };
    if (!chord)
        return null;
    return (_jsx("div", { className: "fixed inset-0 bg-black/60 z-50 flex items-center justify-center", onClick: onClose, children: _jsxs("div", { className: "bg-gray-900 rounded-xl border border-gray-700 p-6 w-96 shadow-2xl overflow-y-auto", style: { maxHeight: '90vh' }, onClick: e => e.stopPropagation(), children: [_jsxs("div", { className: "flex items-center justify-between mb-4", children: [_jsx("button", { onClick: onPrev, disabled: chordIdx <= 0, className: "text-gray-500 hover:text-white text-lg disabled:opacity-30 disabled:cursor-not-allowed", children: "\u25C0" }), _jsxs("div", { className: "flex items-center gap-3 relative", children: [editing ? (_jsxs(_Fragment, { children: [_jsx("input", { ref: inputRef, autoFocus: true, value: editText, onChange: e => {
                                                const val = e.target.value;
                                                setEditText(val);
                                                // Utiliser le helper partagé, avec une chaîne vide comme lastChiffrage
                                                setSuggestions(getAutocompleteSuggestions(val, ''));
                                                setSuggestIdx(0);
                                            }, onKeyDown: e => {
                                                if (suggestions.length > 0) {
                                                    if (e.key === 'Tab' || e.key === 'Enter') {
                                                        e.preventDefault();
                                                        applySuggestion(suggestions[suggestIdx]);
                                                        return;
                                                    }
                                                    if (e.key === 'ArrowDown') {
                                                        e.preventDefault();
                                                        setSuggestIdx(p => Math.min(p + 1, suggestions.length - 1));
                                                        return;
                                                    }
                                                    if (e.key === 'ArrowUp') {
                                                        e.preventDefault();
                                                        setSuggestIdx(p => Math.max(p - 1, 0));
                                                        return;
                                                    }
                                                    if (e.key === 'Escape') {
                                                        setSuggestions([]);
                                                        return;
                                                    }
                                                }
                                                if (e.key === 'Enter')
                                                    commitEdit();
                                                if (e.key === 'Escape') {
                                                    setEditing(false);
                                                    setEditText(`${chord.time}:${chord.chiffrage}`);
                                                    setSuggestions([]);
                                                }
                                            }, onBlur: () => {
                                                setTimeout(() => { setSuggestions([]); }, 200);
                                                commitEdit();
                                            }, className: "bg-gray-800 text-white text-xl font-bold font-mono px-3 py-1 rounded-lg border border-blue-500 outline-none w-40" }), suggestions.length > 0 && (_jsx("div", { className: "absolute top-full left-0 mt-1 bg-gray-800 border border-gray-700 rounded-lg shadow-xl z-50 overflow-hidden min-w-[8rem]", children: suggestions.map((s, i) => (_jsx("div", { onMouseDown: e => { e.preventDefault(); applySuggestion(s); }, className: `px-3 py-1.5 text-sm font-mono cursor-pointer transition-colors ${i === suggestIdx
                                                    ? 'bg-blue-700 text-white'
                                                    : 'text-gray-300 hover:bg-gray-700'}`, children: s }, i))) }))] })) : (_jsxs("h3", { onClick: () => {
                                        setEditing(true);
                                        setEditText(`${chord.time}:${chord.chiffrage}`);
                                    }, className: "text-xl font-bold font-mono text-white cursor-pointer hover:text-blue-400 transition-colors", title: "Cliquer pour modifier", children: [chord.time, ":", chord.chiffrage] })), _jsxs("span", { className: "text-[10px] text-gray-600 font-mono", children: [chordIdx + 1, "/", chordsCount] })] }), _jsx("button", { onClick: onNext, disabled: chordIdx >= chordsCount - 1, className: "text-gray-500 hover:text-white text-lg disabled:opacity-30 disabled:cursor-not-allowed", children: "\u25B6" }), _jsx("button", { onClick: onTogglePlay, disabled: !chord || chord.midiValues.length === 0, className: `text-lg font-bold px-3 py-1 rounded-lg transition-colors ${playing()
                                ? 'bg-red-800 hover:bg-red-700 text-red-300'
                                : 'bg-emerald-800 hover:bg-emerald-700 text-emerald-300'}`, children: playing() ? '■ Arrêter' : '▶ Jouer' }), _jsx("button", { onClick: onClose, className: "text-gray-500 hover:text-white text-lg", children: "\u2715" })] }), _jsxs("div", { className: "grid grid-cols-2 gap-2 mb-4 text-sm", children: [_jsxs("div", { className: "bg-gray-800/60 rounded-lg px-3 py-2", children: [_jsx("div", { className: "text-[10px] text-gray-500 uppercase", children: "Fondamentale" }), _jsx("div", { className: "text-white font-bold font-mono", children: chord.name })] }), _jsxs("div", { className: "bg-gray-800/60 rounded-lg px-3 py-2", children: [_jsx("div", { className: "text-[10px] text-gray-500 uppercase", children: "Qualit\u00E9" }), _jsx("div", { className: "text-cyan-400 font-bold font-mono", children: chord.quality || 'Majeure' })] }), _jsxs("div", { className: "bg-gray-800/60 rounded-lg px-3 py-2", children: [_jsx("div", { className: "text-[10px] text-gray-500 uppercase", children: "Basse" }), _jsx("div", { className: "text-amber-400 font-bold font-mono", children: chord.bass === chord.name ? '(fond.)' : chord.bass })] }), _jsxs("div", { className: "bg-gray-800/60 rounded-lg px-3 py-2", children: [_jsx("div", { className: "text-[10px] text-gray-500 uppercase", children: "Dur\u00E9e" }), _jsx("div", { className: "text-gray-300 font-bold font-mono", children: durationLabel(chord.time) })] })] }), _jsxs("div", { className: "mb-4", children: [_jsx("label", { className: "text-[10px] text-gray-500 uppercase tracking-wider font-bold mb-2 block", children: "Clavier" }), _jsx("div", { className: "bg-gray-800/40 rounded-lg px-2 pt-2 pb-3", children: _jsx(PianoKeyboard, { activeNotes: notesWithOctave(chord) }) })] }), _jsxs("div", { className: "mb-3", children: [_jsx("label", { className: "text-[10px] text-gray-500 uppercase tracking-wider font-bold mb-2 block", children: "Notes" }), _jsx("div", { className: "flex flex-wrap gap-2", children: chord.notes.map((note, ni) => {
                                const rootVal = NOTE_TO_MIDI[chord.name] ?? 0;
                                const noteVal = NOTE_TO_MIDI[note] ?? 0;
                                const interval = ((noteVal - rootVal) % 12 + 12) % 12;
                                const intervalName = INTERVAL_NAMES[interval];
                                return (_jsxs("div", { title: `Intervalle: ${intervalName} (${interval} demi-tons)`, className: "flex flex-col items-center px-3 py-2 rounded-lg border", style: {
                                        backgroundColor: 'rgba(40,40,40,0.8)',
                                        borderColor: 'rgba(60,60,60,0.8)',
                                    }, children: [_jsx("span", { className: "text-sm font-bold font-mono", style: { color: getNoteColor(note) }, children: note }), _jsx("span", { className: "text-[10px] text-gray-500 mt-0.5", children: intervalName })] }, ni));
                            }) })] }), chord.midiValues.length > 0 && (_jsxs("div", { className: "mb-3", children: [_jsx("label", { className: "text-[10px] text-gray-500 uppercase tracking-wider font-bold mb-2 block", children: "MIDI raw" }), _jsxs("div", { className: "flex flex-wrap gap-1.5", children: [chord.midiValues.map((v, i) => (_jsx("span", { className: "px-2 py-1 bg-gray-800 rounded text-[11px] font-mono text-gray-400 border border-gray-700", children: v }, i))), _jsxs("span", { className: "text-[10px] text-gray-600 self-center ml-1", children: ["(+", NOTE_TO_MIDI[chord.name] ?? 0, " racine)"] })] })] })), _jsx("div", { className: "bg-gray-800/50 rounded-lg p-3 mt-2", children: _jsxs("p", { className: "text-[11px] text-gray-400 font-mono leading-relaxed", children: [_jsx("span", { className: "text-blue-400", children: chord.name }), chord.quality && _jsx("span", { className: "text-cyan-400", children: chord.quality }), chord.bass !== chord.name && (_jsxs("span", { className: "text-amber-400", children: ["/", chord.bass] })), ' → ', _jsx("span", { style: { color: getNoteColor(chord.notes[0]) }, children: chord.notes[0] }), chord.notes.slice(1).map((n, i) => (_jsxs("span", { style: { color: getNoteColor(n) }, children: [", ", n] }, i)))] }) }), _jsx("button", { onClick: onClose, className: "w-full mt-4 px-4 py-2 bg-gray-800 hover:bg-gray-700 text-gray-400 text-xs font-bold rounded-lg transition-colors", children: "Fermer" })] }) }));
}
