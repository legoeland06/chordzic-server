import { jsx as _jsx } from "react/jsx-runtime";
/**
 * Tests de rendu du ChordNowModal (cercle translucide — accord en lecture).
 */
import { describe, expect, it } from 'vitest';
import { renderToString } from 'react-dom/server';
import ChordNowModal from './ChordNowModal';
const chords = [
    { time: 4, chiffrage: 'C' },
    { time: 4, chiffrage: 'Am7' },
    { time: 4, chiffrage: '_' },
    { time: 4, chiffrage: 'C/G' },
];
describe('<ChordNowModal />', () => {
    it('ne rend rien hors lecture (pas de modal fantôme)', () => {
        expect(renderToString(_jsx(ChordNowModal, { chords: chords, highlighted: 0, playing: false }))).toBe('');
        expect(renderToString(_jsx(ChordNowModal, { chords: chords, highlighted: -1, playing: true }))).toBe('');
        expect(renderToString(_jsx(ChordNowModal, { chords: [], highlighted: 0, playing: true }))).toBe('');
    });
    it('affiche l accord courant en très gros (cercle translucide)', () => {
        const html = renderToString(_jsx(ChordNowModal, { chords: chords, highlighted: 0, playing: true }));
        expect(html).toContain('rounded-full'); // la modal circulaire
        expect(html).toContain('En lecture');
        expect(html).toContain('>C</div>'); // l'accord courant
        // L'accord suivant est affiché en petit
        expect(html).toContain('Am7');
    });
    it('affiche un tiret pour un silence (_)', () => {
        const html = renderToString(_jsx(ChordNowModal, { chords: chords, highlighted: 2, playing: true }));
        expect(html).toContain('\u2014');
    });
    it('affiche la basse (C/G) telle quelle', () => {
        const html = renderToString(_jsx(ChordNowModal, { chords: chords, highlighted: 3, playing: true }));
        expect(html).toContain('C/G');
    });
    it('pas d accord suivant au dernier index', () => {
        const html = renderToString(_jsx(ChordNowModal, { chords: chords, highlighted: 3, playing: true }));
        expect(html).not.toContain('>Am7<');
    });
});
