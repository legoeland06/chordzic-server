/**
 * pianoNote — LivePiano cliquable : envoie une note (on/off) au Roland.
 *
 * POST /piano-note (note-on à l'appui, note-off au relâchement). `channel`
 * optionnel : canal de la piste cible en mode Navig (le serveur applique
 * aussi le mapping drums natif) ; absent → canal d'écho configuré / 1.
 */
import { backendUrl } from './chordUtils';
export const PIANO_NOTE_VELOCITY = 96;
/** Envoie l'appui (`on=true`) ou le relâchement (`on=false`) d'une touche. */
export async function sendPianoNote(pitch, on, channel, velocity = PIANO_NOTE_VELOCITY) {
    try {
        const res = await fetch(`${backendUrl()}/piano-note`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ pitch, velocity, on, channel }),
        });
        return res.ok;
    }
    catch (e) {
        console.error('piano-note', e);
        return false;
    }
}
