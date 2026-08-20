import { jsx as _jsx } from "react/jsx-runtime";
/**
 * Tests de rendu du composant LivePiano (react-dom/server, sans DOM).
 */
import { describe, expect, it } from 'vitest';
import { renderToString } from 'react-dom/server';
import LivePiano from './LivePiano';
describe('<LivePiano />', () => {
    it('affiche 88 touches par défaut (A0 → C8)', () => {
        const html = renderToString(_jsx(LivePiano, { activePitches: [] }));
        const liCount = (html.match(/<li/g) ?? []).length;
        expect(liCount).toBe(88);
    });
    it('peut couvrir une plage réduite (pitchMin/pitchMax)', () => {
        const html = renderToString(_jsx(LivePiano, { activePitches: [36], pitchMin: 36, pitchMax: 59 }));
        const liCount = (html.match(/<li/g) ?? []).length;
        expect(liCount).toBe(24);
        expect(html).toContain('class="white e active"');
    });
    it('illumine les touches tenues (classe active)', () => {
        // C4 (60) → "white e", E4 (64) → "white c", G4 (67) → "white a"
        const html = renderToString(_jsx(LivePiano, { activePitches: [60, 64, 67] }));
        expect(html).toContain('class="white e active"');
        expect(html).toContain('class="white c active"');
        expect(html).toContain('class="white a active"');
    });
    it('illumine aussi les touches noires', () => {
        // C#4 (61) → "black cs"
        const html = renderToString(_jsx(LivePiano, { activePitches: [61] }));
        expect(html).toContain('class="black cs active"');
        // Le C4 voisin reste inactif
        expect(html).not.toContain('class="white e active"');
    });
    it('ignore les notes hors plage (pas d illumination parasite)', () => {
        const html = renderToString(_jsx(LivePiano, { activePitches: [12, 130, 60] }));
        // Seul C4 (60) illumine
        const activeCount = (html.match(/active/g) ?? []).length;
        expect(activeCount).toBe(1);
    });
    it('affiche un tooltip note+octave sur chaque touche (A0 … C8)', () => {
        const html = renderToString(_jsx(LivePiano, { activePitches: [] }));
        expect(html).toContain('title="A0"');
        expect(html).toContain('title="C8"');
        expect(html).toContain('title="C4"');
    });
});
