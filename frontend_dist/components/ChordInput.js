import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * ChordInput — zone de texte pour la saisie des accords avec autocomplétion.
 *
 * Fonctionnalités :
 * - Saisie libre au format "4:Cm7 2:FM7 4:G7 4:C"
 * - Autocomplétion intelligente avec Tab/Enter/↑/↓
 * - Suggestions basées sur la bibliothèque QUALITY_INTERVALS (70+ qualités)
 * - Détection du token courant à la position du curseur
 * - Propose le dernier accord tapé quand on tape juste "4:"
 *
 * La logique d'autocomplétion est importée depuis lib/autocomplete.ts
 * pour éviter la duplication avec ChordApp.tsx et ChordDetailModal.tsx.
 */
import { useState, useRef, useCallback } from 'react';
import { getCurrentToken, getSuggestions, replaceToken } from '../lib/autocomplete';
export default function ChordInput({ input, onChange }) {
    const inputRef = useRef(null);
    const [lastChiffrage] = useState('');
    const [suggestions, setSuggestions] = useState([]);
    const [suggestIdx, setSuggestIdx] = useState(0);
    const [suggestToken, setSuggestToken] = useState(null);
    /** Applique la suggestion sélectionnée : remplace le token + repositionne le curseur. */
    const applySuggestion = useCallback((suggestion) => {
        if (!suggestToken)
            return;
        const newInput = replaceToken(input, suggestToken.start, suggestToken.end, suggestion);
        onChange(newInput);
        setSuggestions([]);
        setSuggestToken(null);
        const newCursor = suggestToken.start + suggestion.length;
        requestAnimationFrame(() => {
            if (inputRef.current) {
                inputRef.current.focus();
                inputRef.current.setSelectionRange(newCursor, newCursor);
            }
        });
    }, [input, suggestToken, onChange]);
    /** Gère la saisie : met à jour le texte + recalcule les suggestions. */
    const handleInputChange = (e) => {
        const val = e.target.value;
        onChange(val);
        const cursor = e.target.selectionStart ?? val.length;
        const { start, end, token } = getCurrentToken(val, cursor);
        const results = getSuggestions(token, lastChiffrage);
        setSuggestions(results);
        setSuggestIdx(0);
        setSuggestToken(results.length > 0 ? { start, end } : null);
    };
    /** Gère les touches : Tab/Enter valider, ↑↓ navigation, Esc fermer. */
    const handleInputKeyDown = (e) => {
        if (suggestions.length === 0)
            return;
        if (e.key === 'Tab' || e.key === 'Enter') {
            e.preventDefault();
            applySuggestion(suggestions[suggestIdx]);
            return;
        }
        if (e.key === 'ArrowDown') {
            e.preventDefault();
            setSuggestIdx(prev => Math.min(prev + 1, suggestions.length - 1));
            return;
        }
        if (e.key === 'ArrowUp') {
            e.preventDefault();
            setSuggestIdx(prev => Math.max(prev - 1, 0));
            return;
        }
        if (e.key === 'Escape') {
            setSuggestions([]);
            setSuggestToken(null);
            return;
        }
    };
    return (_jsxs("div", { className: "bg-gray-900 rounded-xl border border-gray-800 p-4 mb-4 relative", children: [_jsxs("label", { className: "text-xs text-gray-500 mb-2 block font-mono", children: ["Accords (ex: 4:Cm7 2:FM7 4:G7 4:C) \u2014", ' ', _jsx("span", { className: "text-blue-400", children: "Tab" }), " pour compl\u00E9ter"] }), _jsx("textarea", { ref: inputRef, value: input, onChange: handleInputChange, onKeyDown: handleInputKeyDown, onBlur: () => {
                    setTimeout(() => { setSuggestions([]); setSuggestToken(null); }, 200);
                }, rows: 5, className: "w-full bg-gray-800 text-white text-sm font-mono px-4 py-3 rounded-lg border border-gray-700 focus:border-blue-500 focus:ring-1 focus:ring-blue-500 outline-none resize-none", placeholder: "4:Cm7 2:FM7 4:G7 4:C" }), suggestions.length > 0 && suggestToken && (_jsxs("div", { className: "absolute left-4 z-50 mt-1 bg-gray-800 border border-gray-700 rounded-lg shadow-xl overflow-hidden", style: { top: '100%', minWidth: 160, maxHeight: 280 }, children: [suggestions.map((s, i) => (_jsx("button", { onMouseDown: (e) => { e.preventDefault(); applySuggestion(s); }, className: `w-full text-left px-4 py-2 text-xs font-mono transition-colors ${i === suggestIdx ? 'bg-blue-700 text-white' : 'text-gray-300 hover:bg-gray-700'}`, children: s }, s))), _jsx("div", { className: "px-4 py-1.5 text-[10px] text-gray-500 border-t border-gray-700", children: "\u2191\u2193 naviguer \u00B7 Tab/Enter valider \u00B7 Esc fermer" })] }))] }));
}
