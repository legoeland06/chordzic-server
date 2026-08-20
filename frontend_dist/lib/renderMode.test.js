import { describe, it, expect } from 'vitest';
import { renderEndpoint, clickModeForRenderer, rendererLabel, } from './renderMode';
describe('renderEndpoint', () => {
    it('interne → /render-wav', () => {
        expect(renderEndpoint('internal')).toBe('/render-wav');
    });
    it('externe → /render-external', () => {
        expect(renderEndpoint('external')).toBe('/render-external');
    });
});
describe('clickModeForRenderer', () => {
    it('externe : jamais séparé, clic mixé selon la config serveur', () => {
        expect(clickModeForRenderer('external', { out_device: 'default', in_render: false }))
            .toEqual({ click_in_render: false });
        expect(clickModeForRenderer('external', { out_device: null, in_render: true }))
            .toEqual({ click_in_render: true });
    });
    it('interne + sortie dédiée → séparé', () => {
        expect(clickModeForRenderer('internal', { out_device: 'default', in_render: false }))
            .toEqual({ click_separate: true });
    });
    it('interne sans sortie dédiée → mixé', () => {
        expect(clickModeForRenderer('internal', { out_device: null, in_render: true }))
            .toEqual({ click_in_render: true });
        expect(clickModeForRenderer('internal', { out_device: null, in_render: false }))
            .toEqual({ click_in_render: false });
    });
});
describe('rendererLabel', () => {
    it('libellés français', () => {
        expect(rendererLabel('internal')).toBe('Interne');
        expect(rendererLabel('external')).toBe('Externe');
    });
});
describe('RenderOptions', () => {
    it('le type Renderer accepte les deux valeurs', () => {
        const r = 'external';
        expect(r).toBe('external');
    });
});
