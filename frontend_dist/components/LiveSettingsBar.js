import { jsx as _jsx, jsxs as _jsxs, Fragment as _Fragment } from "react/jsx-runtime";
/**
 * LiveSettingsBar — réglages musicaux compacts, partagés entre les deux
 * modes (Live et Navig) : volume master, 432Hz, Loop, Walking Bass, Pattern
 * drums et signature. Style affiné (finesse des lignes, états colorés
 * discrets) cohérent avec le mode Navig.
 *
 * En mode Navig, `showLoop` est à false (le Loop y est déjà géré par
 * LoopControl + les locators).
 */
import { memo } from 'react';
import { Gauge, Volume2 } from 'lucide-react';
const PATTERNS = [
    { value: 'rock', label: '🎸 Rock' },
    { value: 'pop', label: '🎤 Pop' },
    { value: 'reggae', label: '🌴 Reggae' },
    { value: 'onedrop', label: '⏬ OneDrop' },
    { value: 'bossa', label: '🌊 Bossa' },
    { value: 'jazz', label: '🎷 Jazz' },
];
/** Style des boutons toggle (fins, comme le mode Navig). */
function toggleCls(active, activeCls) {
    return `px-2 py-1 text-[10px] font-bold rounded-md border transition-colors shrink-0 ${active
        ? activeCls
        : 'bg-gray-800/60 border-gray-700/60 text-gray-500 hover:text-gray-300 hover:border-gray-600'}`;
}
const selectCls = 'bg-gray-800/60 text-[10px] px-1.5 py-1 rounded-md border border-gray-700/60 outline-none shrink-0 focus:border-gray-500';
function LiveSettingsBar({ volume, onSetVolume, use432, onSet432, loopOn, onSetLoop, walkingBass, onSetWalkingBass, drumPattern, onSetDrumPattern, sig, onSetSig, playing, showLoop = true, tempo, onTempoChange, }) {
    return (_jsxs("div", { className: "flex flex-wrap items-center gap-x-2.5 gap-y-1.5 text-[10px] text-gray-500", children: [_jsxs("div", { className: "flex items-center gap-1 shrink-0", children: [_jsx(Volume2, { className: "w-3 h-3 text-gray-500" }), _jsx("span", { className: "shrink-0", children: "Vol" }), _jsx("input", { type: "range", min: 10, max: 127, value: volume, onChange: (e) => onSetVolume(parseInt(e.target.value)), className: "w-16 sm:w-20 accent-green-500 shrink-0", title: "Volume master" }), _jsx("span", { className: "w-5 text-right text-gray-400 font-mono shrink-0", children: volume })] }), _jsx("div", { className: "w-px h-4 bg-gray-700/60 shrink-0" }), _jsxs("button", { onClick: () => onSet432(!use432), className: toggleCls(use432, 'bg-yellow-900/40 border-yellow-600/50 text-yellow-300'), title: "Accordage A=432 Hz (au lieu de 440 Hz)", children: ["432Hz ", use432 ? '●' : '○'] }), showLoop && (_jsxs("button", { onClick: () => onSetLoop(!loopOn), disabled: playing, className: `${toggleCls(loopOn, 'bg-purple-900/40 border-purple-500/50 text-purple-300')} disabled:opacity-40`, title: "R\u00E9p\u00E9ter la grille en boucle (d\u00E9sactiv\u00E9 pendant la lecture)", children: ["\uD83D\uDD01 Loop ", loopOn ? '●' : '○'] })), _jsxs("button", { onClick: () => onSetWalkingBass(!walkingBass), className: toggleCls(walkingBass, 'bg-pink-900/40 border-pink-500/50 text-pink-300'), title: "Walking bass : la basse joue 4 notes par mesure au lieu d'une tenue", children: ["\uD83C\uDFB5 WB ", walkingBass ? '●' : '○'] }), _jsx("div", { className: "w-px h-4 bg-gray-700/60 shrink-0" }), _jsxs("div", { className: "flex items-center gap-1 shrink-0", children: [_jsx("span", { className: "shrink-0", children: "Pattern:" }), _jsx("select", { value: drumPattern, onChange: (e) => onSetDrumPattern(e.target.value), className: selectCls, title: "Style de batterie", children: PATTERNS.map(p => _jsx("option", { value: p.value, children: p.label }, p.value)) })] }), _jsxs("div", { className: "flex items-center gap-1 shrink-0", children: [_jsx("span", { className: "shrink-0", children: "Mesure:" }), _jsxs("select", { value: sig, onChange: (e) => onSetSig(e.target.value), className: selectCls, title: "Signature rythmique", children: [_jsx("option", { value: "4/4", children: "4/4" }), _jsx("option", { value: "3/4", children: "3/4" }), _jsx("option", { value: "6/8", children: "6/8" })] })] }), tempo !== undefined && onTempoChange && (_jsxs(_Fragment, { children: [_jsx("div", { className: "w-px h-4 bg-gray-700/60 shrink-0" }), _jsxs("div", { className: "flex items-center gap-1 shrink-0", children: [_jsx(Gauge, { className: "w-3 h-3 text-gray-500" }), _jsx("span", { className: "shrink-0", children: "Tempo:" }), _jsx("input", { type: "range", min: 40, max: 220, value: tempo, onChange: (e) => onTempoChange(parseInt(e.target.value)), className: "w-16 sm:w-20 accent-blue-500 shrink-0", title: "Tempo (40-220 BPM)" }), _jsx("input", { type: "number", value: tempo, onChange: (e) => onTempoChange(parseInt(e.target.value)), className: "w-10 bg-transparent text-[10px] font-bold text-blue-400 outline-none shrink-0", title: "Tempo en BPM (40-220)" })] })] }))] }));
}
export default memo(LiveSettingsBar);
