/// Patterns drums — constantes, identifiants et helpers partagés entre
/// le live (midi.rs) et le render (render.rs).
///
/// Chaque pattern définit les coups joués sur chaque temps et contretemps
/// pour un style musical spécifique : rock, reggae, jazz, pop, bossa, onedrop.
///
/// Les constantes GM (General MIDI) correspondent aux percussions standards
/// sur le canal 10 (index 9 en 0-based).

// ─── Identifiants de pattern ─────────────────────────────────────────
// Utilisés comme valeur atomique dans Live.pattern et pour les switchs
// dans drum_hit() (live) et generate_smf_fmt0() (render).
pub const PAT_ROCK: u8 = 0;     // Rock standard : kick snare kick snare
pub const PAT_REGGAE: u8 = 1;   // Reggae one-drop dérivé
pub const PAT_JAZZ: u8 = 2;     // Ride cymbal为主导, rimshot accents
pub const PAT_POP: u8 = 3;      // Pop : kick+snare+HH plus léger
pub const PAT_BOSSA: u8 = 4;    // Bossa nova : kick syncopé, snare léger
pub const PAT_ONEDROP: u8 = 5;  // One-drop reggae : kick et rimshot décalés

/// Convertit un nom de pattern textuel (en provenance du frontend) en
/// identifiant numérique.  Le défaut ("rock") est utilisé pour toute
/// chaîne non reconnue.
pub fn pat(s: &str) -> u8 {
    match s {
        "reggae" => PAT_REGGAE,
        "jazz" => PAT_JAZZ,
        "pop" => PAT_POP,
        "bossa" => PAT_BOSSA,
        "onedrop" => PAT_ONEDROP,
        _ => PAT_ROCK,
    }
}

// ─── Notes GM Drums ──────────────────────────────────────────────────
// Références General MIDI Level 1 — Drum Key Map.
// Utilisées via le canal 9 (10 en 1-based GM) qui est le kit drums.
pub const DRUM_KICK: u8 = 36;   // Kick (grosse caisse)
pub const DRUM_SNARE: u8 = 38;  // Snare (caisse claire acoustique)
pub const DRUM_RIM: u8 = 37;    // Rimshot (side stick / bord de caisse)
pub const DRUM_HH: u8 = 42;     // Hi-Hat fermée
pub const DRUM_HH_OPEN: u8 = 47; // Hi-Hat ouverte (note 46) est rarement utilisée dans les patterns de base
pub const DRUM_RIDE: u8 = 51;   // Ride cymbal (utilisé dans le jazz)

// ─── Vélocités de base HH ────────────────────────────────────────────
// Valeurs de vélocité de référence pour les hi-hats, utilisées comme
// niveau de base avant scaling par le volume du track.
pub const HH_BEAT: u8 = 80;   // HH sur les temps (fort)
pub const HH_8TH: u8 = 65;    // HH sur les contretemps en 8ème (plus doux)

/// Scale une vélocité par le volume d'un track et le master volume.
///
/// Formule : (vol * base) / 127, clampée à 127.
/// Cela permet de préserver les proportions relatives entre instruments
/// tout en respectant le niveau global choisi par l'utilisateur.
///
/// # Exemples
/// - sc(127, 100) = 100  (pas d'atténuation)
/// - sc(64, 100)  = 50   (moitié du volume)
/// - sc(0, 100)   = 0    (silence)
pub fn sc(vol: u8, base: u8) -> u8 {
    ((vol as u16 * base as u16) / 127).min(127) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pat() {
        assert_eq!(pat("rock"), PAT_ROCK);
        assert_eq!(pat("reggae"), PAT_REGGAE);
        assert_eq!(pat("jazz"), PAT_JAZZ);
        assert_eq!(pat("pop"), PAT_POP);
        assert_eq!(pat("bossa"), PAT_BOSSA);
        assert_eq!(pat("onedrop"), PAT_ONEDROP);
        assert_eq!(pat("inconnu"), PAT_ROCK); // fallback rock
    }

    #[test]
    fn test_sc() {
        // Volume max → vélocité = base
        assert_eq!(sc(127, 100), 100);
        // Moitié du volume → moitié de la vélocité
        assert_eq!(sc(64, 100), 50);
        // Volume nul → silence
        assert_eq!(sc(0, 100), 0);
        // Clamp à 127 si le résultat dépasse
        assert_eq!(sc(200, 100), 127);
    }
}
