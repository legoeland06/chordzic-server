/**
 * Tests du mapping des raccourcis clavier du PianoRoll (pur, sans DOM).
 */
import { describe, expect, it } from 'vitest';
import { isTypingTarget, pianoRollShortcut } from './pianoRollShortcuts';
const act = (key, opts = {}) => pianoRollShortcut({ key, ctrl: opts.ctrl, meta: opts.meta, shift: opts.shift });
describe('pianoRollShortcut', () => {
    it('outils : e = édition, v = sélection', () => {
        expect(act('e')).toBe('tool-edit');
        expect(act('E')).toBe('tool-edit');
        expect(act('v')).toBe('tool-select');
    });
    it('groupes : Ctrl+G grouper, Ctrl+U dégrouper (majuscules incluses)', () => {
        expect(act('g', { ctrl: true })).toBe('group');
        expect(act('G', { ctrl: true })).toBe('group');
        expect(act('u', { ctrl: true })).toBe('ungroup');
    });
    it('g seul n est PAS grouper (zoom horizontal inchangé)', () => {
        expect(act('g')).toBeNull();
        expect(act('h')).toBeNull();
    });
    it('quantiser : q', () => {
        expect(act('q')).toBe('quantize');
    });
    it('lecture : Ctrl+Espace = audio, Shift+Espace = MIDI, Espace seul = null', () => {
        expect(act(' ', { ctrl: true })).toBe('play-audio');
        expect(act(' ', { meta: true })).toBe('play-audio');
        expect(act(' ', { shift: true })).toBe('play-midi');
        expect(act(' ')).toBeNull(); // Espace seul = lecture locale (gérée à part)
    });
    it('REC : *', () => {
        expect(act('*')).toBe('rec');
    });
    it('tête de lecture : 0 = début, 1 = locator L, 2 = locator R', () => {
        expect(act('0')).toBe('go-start');
        expect(act('1')).toBe('go-loc-l');
        expect(act('2')).toBe('go-loc-r');
    });
    it('zoom sur la sélection : o', () => {
        expect(act('o')).toBe('zoom-selection');
    });
    it('touches inconnues / modificateurs seuls → null', () => {
        expect(act('x')).toBeNull();
        expect(act(' ')).toBeNull();
        expect(act('Enter')).toBeNull();
        expect(act('z', { ctrl: true })).toBeNull(); // undo géré ailleurs
        expect(act('c', { ctrl: true })).toBeNull(); // copier géré ailleurs
    });
    it('cible = input/textarea/select/button → ignoré (saisie en cours)', () => {
        for (const tag of ['INPUT', 'TEXTAREA', 'SELECT', 'BUTTON']) {
            expect(pianoRollShortcut({ key: 'e', target: { tagName: tag } })).toBeNull();
            expect(pianoRollShortcut({ key: '*', target: { tagName: tag } })).toBeNull();
            expect(pianoRollShortcut({ key: 'g', ctrl: true, target: { tagName: tag } })).toBeNull();
        }
    });
    it('cible sans tagName (null/objet) → pas de garde', () => {
        expect(pianoRollShortcut({ key: '0' })).toBe('go-start');
        expect(pianoRollShortcut({ key: 'o', target: null })).toBe('zoom-selection');
    });
    it('exhaustivité : chaque action a au moins un déclencheur', () => {
        const triggered = new Set();
        for (const key of ['e', 'v', 'q', '*', '0', '1', '2', 'o']) {
            const a = act(key);
            if (a)
                triggered.add(a);
        }
        for (const [key, mod] of [['g', true], ['u', true]]) {
            const a = act(key, { ctrl: mod });
            if (a)
                triggered.add(a);
        }
        triggered.add(act(' ', { ctrl: true }));
        triggered.add(act(' ', { shift: true }));
        const all = [
            'tool-edit', 'tool-select', 'group', 'ungroup', 'quantize',
            'rec', 'go-start', 'go-loc-l', 'go-loc-r', 'zoom-selection',
            'play-audio', 'play-midi',
        ];
        for (const a of all)
            expect(triggered.has(a), a).toBe(true);
    });
});
describe('isTypingTarget', () => {
    it('détecte les champs de saisie', () => {
        expect(isTypingTarget({ tagName: 'INPUT' })).toBe(true);
        expect(isTypingTarget({ tagName: 'DIV' })).toBe(false);
        expect(isTypingTarget(null)).toBe(false);
        expect(isTypingTarget(undefined)).toBe(false);
    });
});
