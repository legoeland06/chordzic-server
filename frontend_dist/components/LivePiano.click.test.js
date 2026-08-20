import { jsx as _jsx } from "react/jsx-runtime";
/**
 * Tests du LivePiano CLIQUABLE (pointer events, environnement jsdom).
 *
 * jsdom n'implémente pas la pointer capture : on la stubbe sur les touches.
 */
// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import LivePiano from './LivePiano';
// jsdom n'a ni pointer capture (stubbée par touche) ni ResizeObserver :
// no-op globaux pour le fit scale.
class MockResizeObserver {
    observe() { }
    unobserve() { }
    disconnect() { }
}
globalThis.ResizeObserver = MockResizeObserver;
/** jsdom ne connaît pas la pointer capture — no-op, "capturé". */
function stubPointerCapture(el) {
    el.setPointerCapture = () => { };
    el.releasePointerCapture = () => { };
    el.hasPointerCapture = () => true;
}
function pointer(el, type) {
    el.dispatchEvent(new PointerEvent(type, { bubbles: true, pointerId: 1 }));
}
function renderPiano(onPlayNote) {
    const root = createRoot(document.body);
    act(() => {
        root.render(_jsx(LivePiano, { activePitches: [], onPlayNote: onPlayNote }));
    });
    return root;
}
describe('<LivePiano /> cliquable', () => {
    beforeEach(() => {
        // Le note-off d'un clic court est DIFFÉRÉ (~300 ms) : timers factices.
        vi.useFakeTimers();
    });
    afterEach(() => {
        vi.useRealTimers();
        document.body.innerHTML = '';
    });
    it('appui = note-on, relâchement = note-off (C4 → pitch 60)', () => {
        const onPlayNote = vi.fn();
        const root = renderPiano(onPlayNote);
        const c4 = document.querySelector('li[title="C4"]');
        stubPointerCapture(c4);
        act(() => pointer(c4, 'pointerdown'));
        expect(onPlayNote).toHaveBeenLastCalledWith(60, true);
        act(() => pointer(c4, 'pointerup'));
        // Clic court → note-off différé (la note sonne ~300 ms) : pas encore.
        expect(onPlayNote).toHaveBeenCalledTimes(1);
        act(() => vi.advanceTimersByTime(320));
        expect(onPlayNote).toHaveBeenLastCalledWith(60, false);
        expect(onPlayNote).toHaveBeenCalledTimes(2);
        act(() => root.unmount());
    });
    it('illumine la touche tenue localement (classe active sans /live-input)', () => {
        const onPlayNote = vi.fn();
        const root = renderPiano(onPlayNote);
        const c4 = document.querySelector('li[title="C4"]');
        stubPointerCapture(c4);
        expect(c4.className).not.toContain('active');
        act(() => pointer(c4, 'pointerdown'));
        expect(c4.className).toContain('active');
        act(() => pointer(c4, 'pointerup'));
        expect(c4.className).not.toContain('active');
        act(() => root.unmount());
    });
    it('multi-touches : deux notes tenues en même temps (accord)', () => {
        const onPlayNote = vi.fn();
        const root = renderPiano(onPlayNote);
        const c4 = document.querySelector('li[title="C4"]');
        const e4 = document.querySelector('li[title="E4"]');
        stubPointerCapture(c4);
        stubPointerCapture(e4);
        act(() => pointer(c4, 'pointerdown'));
        act(() => pointer(e4, 'pointerdown'));
        expect(onPlayNote).toHaveBeenCalledWith(60, true);
        expect(onPlayNote).toHaveBeenCalledWith(64, true);
        act(() => pointer(c4, 'pointerup'));
        act(() => pointer(e4, 'pointerup'));
        act(() => vi.advanceTimersByTime(320));
        expect(onPlayNote).toHaveBeenCalledWith(60, false);
        expect(onPlayNote).toHaveBeenCalledWith(64, false);
        act(() => root.unmount());
    });
    it('pointercancel coupe aussi la note (sécurité)', () => {
        const onPlayNote = vi.fn();
        const root = renderPiano(onPlayNote);
        const c4 = document.querySelector('li[title="C4"]');
        stubPointerCapture(c4);
        act(() => pointer(c4, 'pointerdown'));
        act(() => pointer(c4, 'pointercancel'));
        act(() => vi.advanceTimersByTime(320));
        expect(onPlayNote).toHaveBeenLastCalledWith(60, false);
        act(() => root.unmount());
    });
    it('re-clic pendant le délai : le note-off différé est annulé (pas de coupure)', () => {
        const onPlayNote = vi.fn();
        const root = renderPiano(onPlayNote);
        const c4 = document.querySelector('li[title="C4"]');
        stubPointerCapture(c4);
        act(() => pointer(c4, 'pointerdown'));
        act(() => pointer(c4, 'pointerup')); // clic court → note-off dans 300 ms
        act(() => vi.advanceTimersByTime(100));
        act(() => pointer(c4, 'pointerdown')); // re-clic : annule le timer
        act(() => pointer(c4, 'pointerup'));
        act(() => vi.advanceTimersByTime(320));
        // (on, on, off) — jamais d'off intercalé qui couperait la note
        expect(onPlayNote.mock.calls.map(c => c[1])).toEqual([true, true, false]);
        act(() => root.unmount());
    });
    it('sans onPlayNote : aucun handler, pas de note', () => {
        const root = createRoot(document.body);
        act(() => {
            root.render(_jsx(LivePiano, { activePitches: [] }));
        });
        const c4 = document.querySelector('li[title="C4"]');
        stubPointerCapture(c4);
        act(() => pointer(c4, 'pointerdown'));
        act(() => root.unmount());
        expect(true).toBe(true); // pas de throw → aucun handler attaché
    });
});
