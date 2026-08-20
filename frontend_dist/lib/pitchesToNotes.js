/** Vélocité par défaut des notes insérées. */
export const DEFAULT_VELOCITY = 80;
/**
 * Construit les notes de piano roll à partir des pitchs RÉELLEMENT joués
 * (état `active` de /live-input, ordre d'appui conservé par le serveur).
 * Une note par pitch, même durée/position/vélocité, ids uniques — l'ordre
 * du tableau d'entrée est préservé dans le tableau de sortie.
 */
export function pitchesToPianoNotes(pitches, startBeats, durationBeats, velocity = DEFAULT_VELOCITY) {
    const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 7)}`;
    return pitches.map((pitch, i) => ({
        id: `live-${stamp}-${i}`,
        startTime: startBeats,
        pitch,
        duration: durationBeats,
        velocity,
    }));
}
/**
 * Pitchs des notes ACTIVES à une position donnée (en beats) — une note est
 * active quand `startTime <= pos < startTime + duration`. C'est ce qui
 * alimente l'illumination du piano en mode Navig, fidèle au contenu de la
 * piste jouée (que la lecture soit WAV ou MIDI : même tête de lecture).
 */
export function activePitchesAt(notes, posBeats) {
    return notes
        .filter(n => posBeats >= n.startTime && posBeats < n.startTime + n.duration)
        .map(n => n.pitch);
}
