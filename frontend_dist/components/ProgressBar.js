import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
export default function ProgressBar({ chords, highlighted, playing, currentBeat, tempo }) {
    // Pas de barre si pas de lecture ou pas d'accords
    if (chords.length === 0 || !playing)
        return null;
    return (_jsxs("div", { className: "bg-gray-900 rounded-xl border border-gray-800 p-3 mb-2", children: [_jsxs("div", { className: "flex items-center gap-3", children: [_jsx("div", { className: "flex-1 h-2.5 bg-gray-800 rounded-full overflow-hidden", children: _jsx("div", { className: "h-full bg-gradient-to-r from-blue-500 to-blue-400 rounded-full transition-all duration-300 ease-linear", style: { width: `${Math.round(((highlighted + 1) / chords.length) * 100)}%` } }) }), _jsxs("span", { className: "text-[10px] text-gray-500 font-mono shrink-0", children: [Math.round(((highlighted + 1) / chords.length) * 100), "%"] }), _jsxs("span", { className: "text-[10px] text-gray-600 font-mono shrink-0", children: [highlighted + 1, "/", chords.length] })] }), _jsxs("div", { className: "flex items-center justify-center gap-2 mt-2", children: [[0, 1, 2, 3].map(b => (_jsx("div", { className: `w-5 h-5 rounded-full flex items-center justify-center text-[9px] font-bold transition-all duration-100 ${currentBeat === b
                            ? (b === 0
                                ? 'bg-blue-500 text-white scale-110' // Temps 1 = bleu + grossi
                                : 'bg-gray-600 text-white') // Autres temps = gris clair
                            : 'bg-gray-800 text-gray-600' // Inactif
                        }`, children: b + 1 }, b))), _jsxs("span", { className: "text-[10px] text-gray-600 ml-1", children: [tempo, " bpm"] })] })] }));
}
