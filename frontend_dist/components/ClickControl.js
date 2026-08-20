import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * Contrôle de la piste de clic — MODE NAVIG (rendu WAV).
 * Deux modes au choix :
 *  - « Dans le rendu » : clic MÉLANGÉ au WAV → synchro échantillon-parfaite.
 *  - « Sortie » : clic joué SÉPARÉMENT par le serveur sur un appareil
 *    MULTICANAL (agrégat CoreAudio : sortie intégrée + hub) — main ch1-2,
 *    clic ch3-4, UNE seule horloge → synchro échantillon-parfaite aussi.
 * L'état vit côté SERVEUR (/click) — source de vérité unique au moment du
 * rendu (plus aucun aller-retour de mode nécessaire).
 */
import { Metronome, Volume2, VolumeX } from 'lucide-react';
import { useEffect, useState } from 'react';
import { setClickSig } from '../lib/clickPrefs';
import { backendUrl } from '../lib/chordUtils';
const API_BASE = backendUrl();
export default function ClickControl() {
    const [cfg, setCfg] = useState(null);
    const [sounds, setSounds] = useState([]);
    const [devices, setDevices] = useState([]);
    /** Dernier volume non nul (pour restaurer après un mute). */
    const [lastVol, setLastVol] = useState(80);
    useEffect(() => {
        fetch(`${API_BASE}/click`)
            .then((r) => r.json())
            .then((d) => {
            setCfg({
                volume: d.volume, accent: d.accent, sound: d.sound,
                in_render: d.in_render, out_device: d.out_device || null, delay_ms: d.delay_ms || 0,
            });
            setSounds(d.sounds || []);
            if (d.volume > 0)
                setLastVol(d.volume);
        })
            .catch(() => { });
        fetch(`${API_BASE}/audio-devices`)
            .then((r) => r.json())
            .then((d) => setDevices(d.devices || []))
            .catch(() => { });
    }, []);
    const save = (c) => {
        setClickSig(JSON.stringify(c)); // signature pour forcer le re-rendu au Play
        fetch(`${API_BASE}/click`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ ...c, out_device: c.out_device || '' }),
        }).catch(() => { });
    };
    const apply = (patch) => {
        setCfg((prev) => {
            const next = {
                ...(prev || { volume: 80, accent: true, sound: 0, in_render: false, out_device: null, delay_ms: 0 }),
                ...patch,
            };
            save(next);
            return next;
        });
    };
    const onRenderToggle = (v) => {
        apply({ in_render: v });
    };
    const onDeviceChange = (name) => {
        // Choisir une sortie = mode séparé → on décoche le mix
        apply({ out_device: name || null, in_render: false });
    };
    const onMuteToggle = () => {
        if ((cfg?.volume ?? 0) === 0) {
            apply({ volume: lastVol || 80 });
        }
        else {
            if (cfg)
                setLastVol(cfg.volume);
            apply({ volume: 0 });
        }
    };
    if (!cfg)
        return null;
    const separated = !!cfg.out_device;
    return (_jsxs("div", { className: "flex items-center gap-1.5 shrink-0 px-1 py-1 rounded-lg border border-gray-800 bg-gray-900/60", children: [_jsx(Metronome, { className: "w-3.5 h-3.5 text-amber-400 shrink-0" }), _jsxs("label", { title: "Int\u00E8gre le clic au WAV rendu (mode Navig) \u2014 synchronisation parfaite par construction. Le clic sort avec le son principal.", className: "flex items-center gap-1 text-[10px] text-gray-300 cursor-pointer", children: [_jsx("input", { type: "checkbox", checked: cfg.in_render, onChange: (e) => onRenderToggle(e.target.checked), className: "accent-amber-500" }), "Dans le rendu"] }), _jsxs("select", { value: cfg.out_device || '', onChange: (e) => onDeviceChange(e.target.value), title: "Sortie audio d\u00E9di\u00E9e au clic (mode s\u00E9par\u00E9) : le serveur joue le clic sur CETTE sortie pendant que le navigateur joue le son principal. Ex : le hub USB-C sur Mac.", className: "bg-gray-800 text-gray-300 text-xs rounded-md px-1.5 py-1.5 max-w-[130px] border border-gray-700", children: [_jsx("option", { value: "", children: "Sortie : \u2014" }), devices.map((d) => (_jsxs("option", { value: d.name, children: [d.name, " (", d.channels, "ch)"] }, d.name)))] }), _jsx("select", { value: cfg.sound, onChange: (e) => apply({ sound: parseInt(e.target.value) }), title: "Son du clic", className: "bg-gray-800 text-gray-300 text-xs rounded-md px-1.5 py-1.5 border border-gray-700", children: sounds.map((s) => (_jsx("option", { value: s.id, children: s.name }, s.id))) }), _jsx("button", { onClick: onMuteToggle, title: cfg.volume === 0 ? 'Réactiver le clic' : 'Couper le clic (mute)', className: `w-6 h-6 flex items-center justify-center rounded-md transition-colors shrink-0 ${cfg.volume === 0 ? 'bg-[#8f3b3b] text-white' : 'text-amber-400 hover:bg-[#1a2230]'}`, children: cfg.volume === 0 ? _jsx(VolumeX, { className: "w-3.5 h-3.5" }) : _jsx(Volume2, { className: "w-3.5 h-3.5" }) }), _jsx("input", { type: "range", min: 0, max: 100, value: cfg.volume, onChange: (e) => { const v = parseInt(e.target.value); if (v > 0)
                    setLastVol(v); apply({ volume: v }); }, title: `Volume du clic (${cfg.volume})`, className: "w-12 accent-amber-500" }), separated && (_jsxs("div", { className: "flex items-center gap-1", title: `Décalage clic (${cfg.delay_ms} ms) — si le clic sort EN AVANCE (chemin USB direct vs PipeWire), augmentez (+) ; s'il sort EN RETARD, diminuez (−). Modifiable pendant la lecture (WAV séparé ET MIDI).`, children: [_jsx("span", { className: "text-[10px] text-gray-400", children: "D\u00E9calage" }), _jsx("input", { type: "range", min: -200, max: 200, step: 1, value: cfg.delay_ms, onChange: (e) => apply({ delay_ms: parseInt(e.target.value) }), className: "w-28 accent-amber-500" }), _jsx("input", { type: "number", min: -200, max: 200, step: 1, value: cfg.delay_ms, onChange: (e) => apply({ delay_ms: Math.max(-200, Math.min(200, parseInt(e.target.value) || 0)) }), className: "w-11 bg-gray-800 text-gray-200 text-xs rounded-md px-1 py-1 border border-gray-700 text-center" }), _jsx("span", { className: "text-[10px] text-gray-500", children: "ms" }), _jsx("button", { onClick: () => apply({ delay_ms: Math.max(-200, cfg.delay_ms - 10) }), className: "px-1.5 py-0.5 bg-gray-800 hover:bg-gray-700 text-gray-300 text-xs rounded border border-gray-700", title: "\u221210 ms", children: "\u221210" }), _jsx("button", { onClick: () => apply({ delay_ms: Math.max(-200, cfg.delay_ms - 1) }), className: "px-1.5 py-0.5 bg-gray-800 hover:bg-gray-700 text-gray-300 text-xs rounded border border-gray-700", title: "\u22121 ms", children: "\u22121" }), _jsx("button", { onClick: () => apply({ delay_ms: Math.min(200, cfg.delay_ms + 1) }), className: "px-1.5 py-0.5 bg-gray-800 hover:bg-gray-700 text-gray-300 text-xs rounded border border-gray-700", title: "+1 ms", children: "+1" }), _jsx("button", { onClick: () => apply({ delay_ms: Math.min(200, cfg.delay_ms + 10) }), className: "px-1.5 py-0.5 bg-gray-800 hover:bg-gray-700 text-gray-300 text-xs rounded border border-gray-700", title: "+10 ms", children: "+10" })] })), _jsxs("label", { title: "Accent sur le 1er temps de chaque mesure", className: "flex items-center gap-1 text-[10px] text-gray-400 cursor-pointer", children: [_jsx("input", { type: "checkbox", checked: cfg.accent, onChange: (e) => apply({ accent: e.target.checked }), className: "accent-amber-500" }), "Accent"] })] }));
}
