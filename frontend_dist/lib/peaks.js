/**
 * peaks — calcul décimé des peaks min/max d'un AudioBuffer (rendu waveform).
 *
 * Les peaks sont calculés UNE FOIS par buffer (coût unique au chargement),
 * puis le canvas ne dessine que les buckets visibles selon le zoom.
 */
/**
 * Calcule les peaks min/max d'un ensemble de canaux (fonction PURE, testable
 * sans Web Audio). `channels` : un Float32Array par canal ; `sampleLength` :
 * nombre d'échantillons (max des canaux) ; `duration` : durée en secondes.
 */
export function computePeaksFromChannels(channels, sampleLength, duration, bucketsPerSec = 60) {
    const buckets = Math.max(64, Math.ceil(duration * bucketsPerSec));
    const min = new Float32Array(buckets);
    const max = new Float32Array(buckets);
    for (let i = 0; i < buckets; i++) {
        min[i] = 1;
        max[i] = -1;
    }
    const chCount = Math.max(1, channels.length);
    const per = Math.max(1, Math.floor(sampleLength / buckets));
    for (let b = 0; b < buckets; b++) {
        const start = b * per;
        const end = Math.min(sampleLength, start + per);
        let mn = 1, mx = -1;
        for (let i = start; i < end; i++) {
            for (let c = 0; c < chCount; c++) {
                const v = channels[c][i];
                if (v < mn)
                    mn = v;
                if (v > mx)
                    mx = v;
            }
        }
        min[b] = mn;
        max[b] = mx;
    }
    return { min, max, buckets, bucketsPerSec, duration };
}
export function computePeaks(buffer, bucketsPerSec = 60) {
    const chData = [];
    for (let c = 0; c < buffer.numberOfChannels; c++)
        chData.push(buffer.getChannelData(c));
    return computePeaksFromChannels(chData, buffer.length, buffer.duration, bucketsPerSec);
}
