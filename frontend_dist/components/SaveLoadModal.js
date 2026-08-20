import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * SaveLoadModal — modals de sauvegarde et chargement des grilles.
 *
 * Deux modals indépendants, contrôlés par des props booléennes :
 * - `<SaveModal>` : saisie du nom + bouton sauvegarder
 * - `<LoadModal>` : liste des grilles sauvegardées + charger/supprimer
 *
 * Les données sont persistées via localStorage dans ChordApp.
 */
import { useState } from 'react';
// ─── SaveModal ──────────────────────────────────────────────────────────
export function SaveModal({ show, onClose, onSave, title, placeholder, buttonLabel }) {
    const [name, setName] = useState('');
    if (!show)
        return null;
    return (_jsx("div", { className: "fixed inset-0 bg-black/60 z-50 flex items-center justify-center", onClick: onClose, children: _jsxs("div", { className: "bg-gray-900 rounded-xl border border-gray-700 p-6 w-full max-w-80 mx-4 shadow-2xl", onClick: e => e.stopPropagation(), children: [_jsx("h3", { className: "text-sm font-bold text-white mb-3", children: title ?? '💾 Sauvegarder la grille' }), _jsx("input", { autoFocus: true, value: name, onChange: e => setName(e.target.value), onKeyDown: e => {
                        if (e.key === 'Enter') {
                            onSave(name);
                            setName('');
                        }
                        if (e.key === 'Escape')
                            onClose();
                    }, className: "w-full bg-gray-800 text-white text-sm font-mono px-3 py-2 rounded-lg border border-gray-700 focus:border-blue-500 outline-none mb-4", placeholder: placeholder ?? 'Nom de la grille' }), _jsxs("div", { className: "flex gap-2", children: [_jsx("button", { onClick: () => { onSave(name); setName(''); }, disabled: !name.trim(), className: "flex-1 px-4 py-2 bg-emerald-700 hover:bg-emerald-600 disabled:bg-gray-800 disabled:text-gray-600 text-white text-xs font-bold rounded-lg transition-colors", children: buttonLabel ?? 'Sauvegarder' }), _jsx("button", { onClick: onClose, className: "px-4 py-2 bg-gray-800 hover:bg-gray-700 text-gray-400 text-xs font-bold rounded-lg transition-colors", children: "Annuler" })] })] }) }));
}
// ─── LoadModal ──────────────────────────────────────────────────────────
export function LoadModal({ show, onClose, grilles, onLoad, onDelete }) {
    if (!show)
        return null;
    return (_jsx("div", { className: "fixed inset-0 bg-black/60 z-50 flex items-center justify-center", onClick: onClose, children: _jsxs("div", { className: "bg-gray-900 rounded-xl border border-gray-700 p-6 w-full max-w-96 mx-4 shadow-2xl max-h-[70vh] flex flex-col", onClick: e => e.stopPropagation(), children: [_jsx("h3", { className: "text-sm font-bold text-white mb-3", children: "\uD83D\uDCC2 Grilles sauvegard\u00E9es" }), grilles.length === 0 ? (_jsx("p", { className: "text-gray-500 text-xs py-6 text-center", children: "Aucune grille sauvegard\u00E9e" })) : (_jsx("div", { className: "flex-1 overflow-y-auto space-y-1", children: grilles.map((g) => (_jsxs("div", { className: "flex items-center gap-2 px-3 py-2 rounded-lg hover:bg-gray-800 cursor-pointer group", onClick: () => onLoad(g), children: [_jsxs("div", { className: "flex-1 min-w-0", children: [_jsx("div", { className: "text-sm font-bold text-cyan-400 truncate", children: g.name }), _jsxs("div", { className: "text-[10px] text-gray-500 truncate", children: [g.input, " \u00B7 ", g.tempo, "bpm"] })] }), _jsx("div", { className: "text-[10px] text-gray-600 hidden group-hover:block", children: typeof g.date === 'number'
                                    ? new Date(g.date * 1000).toLocaleString('fr-FR')
                                    : g.date }), _jsx("button", { onClick: e => { e.stopPropagation(); onDelete(g.file ?? g.name); }, className: "text-gray-400 hover:text-red-400 text-sm transition-colors", title: "Supprimer cette grille", children: "\u2715" })] }, g.file ?? g.name))) })), _jsx("button", { onClick: onClose, className: "mt-3 px-4 py-2 bg-gray-800 hover:bg-gray-700 text-gray-400 text-xs font-bold rounded-lg transition-colors", children: "Fermer" })] }) }));
}
/** Confirmation avant « Nouveau projet » : action destructive, jamais
 * exécutée sans accord explicite (même règle que la suppression de piste). */
export function NewProjectModal({ show, onClose, onConfirm }) {
    if (!show)
        return null;
    return (_jsx("div", { className: "fixed inset-0 bg-black/60 z-50 flex items-center justify-center", onClick: onClose, children: _jsxs("div", { className: "bg-gray-900 rounded-xl border border-gray-700 p-6 w-full max-w-80 mx-4 shadow-2xl", onClick: e => e.stopPropagation(), children: [_jsx("h3", { className: "text-sm font-bold text-white mb-2", children: "\u2728 Nouveau projet" }), _jsx("p", { className: "text-xs text-gray-400 mb-1", children: "Effacer le projet courant et repartir de z\u00E9ro ?" }), _jsx("p", { className: "text-[10px] text-gray-500 mb-4", children: "La grille, les pistes, les notes des piano rolls et tous les r\u00E9glages seront r\u00E9initialis\u00E9s. L'auto-sauvegarde locale sera purg\u00E9e (un rechargement ne restaurera pas l'ancien projet)." }), _jsxs("div", { className: "flex gap-2", children: [_jsx("button", { onClick: onConfirm, className: "flex-1 px-4 py-2 bg-red-800 hover:bg-red-700 text-white text-xs font-bold rounded-lg transition-colors", children: "\u2728 Nouveau projet" }), _jsx("button", { onClick: onClose, className: "px-4 py-2 bg-gray-800 hover:bg-gray-700 text-gray-400 text-xs font-bold rounded-lg transition-colors", children: "Annuler" })] })] }) }));
}
