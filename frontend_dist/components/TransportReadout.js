import { jsx as _jsx, jsxs as _jsxs, Fragment as _Fragment } from "react/jsx-runtime";
/**
 * TransportReadout — afficheurs Mesure / Temps / Durée du transport.
 *
 * S'abonne au store `playhead` (~10 fps) et se re-rend SEUL : pendant la
 * lecture, le transport et le DAW ne re-rendent plus à chaque tick
 * (optimisation performance B).
 */
import { useEffect, useState } from 'react';
import { getPlayheadPosition } from '../lib/playhead';
const tLcd = 'flex flex-col items-center justify-center px-2 py-0.5 bg-[#0a0c10] border border-[#1f2733] rounded-md min-w-[3.2rem] shrink-0';
const tLcdLabel = 'text-[8px] uppercase tracking-widest text-[#5c6472] leading-none';
const tLcdVal = 'font-mono text-[12px] text-[#d9b25f] leading-tight';
function fmtTime(sec) {
    if (!Number.isFinite(sec) || sec < 0)
        sec = 0;
    const m = Math.floor(sec / 60);
    const s = Math.floor(sec % 60);
    const d = Math.floor((sec % 1) * 10);
    return `${m}:${String(s).padStart(2, '0')}.${d}`;
}
export default function TransportReadout({ beatsPerBar, tempo, durSec }) {
    const [pos, setPos] = useState(0);
    useEffect(() => {
        setPos(getPlayheadPosition());
        const id = setInterval(() => setPos(getPlayheadPosition()), 100);
        return () => clearInterval(id);
    }, []);
    const measure = Math.floor(pos / beatsPerBar) + 1;
    const beatInBar = Math.floor(pos % beatsPerBar) + 1;
    const elapsedSec = (pos * 60) / Math.max(40, tempo);
    return (_jsxs(_Fragment, { children: [_jsxs("div", { className: tLcd, title: "Mesure courante \u00B7 temps dans la mesure", children: [_jsx("span", { className: tLcdLabel, children: "Mes." }), _jsxs("span", { className: tLcdVal, children: [String(measure).padStart(3, '0'), ".", beatInBar] })] }), _jsxs("div", { className: tLcd, title: "Temps \u00E9coul\u00E9 depuis le d\u00E9but", children: [_jsx("span", { className: tLcdLabel, children: "Temps" }), _jsx("span", { className: tLcdVal, children: fmtTime(elapsedSec) })] }), _jsxs("div", { className: tLcd, title: "Dur\u00E9e totale du morceau", children: [_jsx("span", { className: tLcdLabel, children: "Dur\u00E9e" }), _jsx("span", { className: tLcdVal, children: fmtTime(durSec) })] })] }));
}
