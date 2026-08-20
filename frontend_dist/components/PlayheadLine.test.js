import { jsx as _jsx } from "react/jsx-runtime";
/**
 * Tests de PlayheadLine (environnement jsdom).
 *
 * Reproduit le bug « homothétie » : à la fermeture du PianoRoll intégré, le
 * TrackLane compact se remonte avec une largeur mesurée APRÈS coup (200 px
 * puis la vraie) → l'échelle change sans notification du store → la ligne
 * restait sur l'ancienne échelle (position × 200/totalBeats) jusqu'au
 * prochain mouvement de la tête.
 */
// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import PlayheadLine from './PlayheadLine';
import { resetPlayhead, setPlayheadPosition } from '../lib/playhead';
function lineEl() {
    const el = document.querySelector('div[style*="translateX"]');
    if (!el)
        throw new Error('ligne de lecture introuvable');
    return el;
}
function renderLine(scale, contentWidth) {
    const root = createRoot(document.body);
    act(() => {
        root.render(_jsx(PlayheadLine, { scale: scale, contentWidth: contentWidth }));
    });
    return root;
}
describe('<PlayheadLine />', () => {
    beforeEach(() => {
        vi.useFakeTimers();
        resetPlayhead();
    });
    afterEach(() => {
        vi.useRealTimers();
        document.body.innerHTML = '';
    });
    it('positionne la ligne à position × scale', () => {
        renderLine(2);
        act(() => { setPlayheadPosition(10); });
        act(() => { vi.advanceTimersByTime(16); });
        expect(lineEl().style.transform).toBe('translateX(20px)');
    });
    it("recalcule la position quand l'échelle change SANS notification du store", () => {
        const root = renderLine(2);
        act(() => { setPlayheadPosition(10); });
        act(() => { vi.advanceTimersByTime(16); });
        expect(lineEl().style.transform).toBe('translateX(20px)');
        // L'échelle change (TrackLane remesuré) : AUCUNE nouvelle position
        // n'est publiée — la ligne doit quand même se repositionner.
        act(() => {
            root.render(_jsx(PlayheadLine, { scale: 4 }));
        });
        expect(lineEl().style.transform).toBe('translateX(40px)');
        act(() => root.unmount());
    });
    it('recalcule quand la largeur du contenu change (masquage hors champ)', () => {
        const root = renderLine(1, 50);
        act(() => { setPlayheadPosition(100); });
        act(() => { vi.advanceTimersByTime(16); });
        expect(lineEl().style.visibility).toBe('hidden');
        act(() => {
            root.render(_jsx(PlayheadLine, { scale: 1, contentWidth: 500 }));
        });
        expect(lineEl().style.transform).toBe('translateX(100px)');
        expect(lineEl().style.visibility).toBe('visible');
        act(() => root.unmount());
    });
});
