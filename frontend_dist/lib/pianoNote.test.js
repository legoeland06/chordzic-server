/**
 * Tests de sendPianoNote (LivePiano cliquable → POST /piano-note).
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { sendPianoNote } from './pianoNote';
describe('sendPianoNote', () => {
    const fetchMock = vi.fn();
    afterEach(() => {
        vi.unstubAllGlobals();
        fetchMock.mockReset();
    });
    it("appui : POST /piano-note avec note-on (pitch, vélocité)", async () => {
        fetchMock.mockResolvedValue({ ok: true });
        vi.stubGlobal('fetch', fetchMock);
        const ok = await sendPianoNote(60, true);
        expect(ok).toBe(true);
        const [url, init] = fetchMock.mock.calls[0];
        expect(String(url)).toMatch(/\/piano-note$/);
        expect(JSON.parse(init.body)).toEqual({ pitch: 60, velocity: 96, on: true, channel: undefined });
    });
    it('relâchement : on=false, canal fourni transmis', async () => {
        fetchMock.mockResolvedValue({ ok: true });
        vi.stubGlobal('fetch', fetchMock);
        await sendPianoNote(64, false, 2, 100);
        const [, init] = fetchMock.mock.calls[0];
        expect(JSON.parse(init.body)).toEqual({ pitch: 64, velocity: 100, on: false, channel: 2 });
    });
    it("échec réseau → false (jamais de throw)", async () => {
        fetchMock.mockRejectedValue(new Error('connexion morte'));
        vi.stubGlobal('fetch', fetchMock);
        expect(await sendPianoNote(60, true)).toBe(false);
    });
    it('réponse non-ok → false', async () => {
        fetchMock.mockResolvedValue({ ok: false });
        vi.stubGlobal('fetch', fetchMock);
        expect(await sendPianoNote(60, true)).toBe(false);
    });
});
