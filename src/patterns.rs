/// Patterns drums — constantes et helpers partagés entre le live (main.rs)
/// et le render (render.rs).
// ─── Identifiants de pattern ─────────────────────────────────────────
pub const PAT_ROCK: u8 = 0;
pub const PAT_REGGAE: u8 = 1;
pub const PAT_JAZZ: u8 = 2;
pub const PAT_POP: u8 = 3;
pub const PAT_BOSSA: u8 = 4;
pub const PAT_ONEDROP: u8 = 5;

/// Convertit un nom de pattern en identifiant.
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
pub const DRUM_KICK: u8 = 36;
pub const DRUM_SNARE: u8 = 38;
pub const DRUM_RIM: u8 = 37;
pub const DRUM_HH: u8 = 42;
pub const DRUM_RIDE: u8 = 51;

// ─── Vélocités de base HH ────────────────────────────────────────────
pub const HH_BEAT: u8 = 80;
pub const HH_8TH: u8 = 65;

/// Scale une vélocité par le volume d'un track.
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
        assert_eq!(sc(127, 100), 100);
        assert_eq!(sc(64, 100), 50);
        assert_eq!(sc(0, 100), 0);
        assert_eq!(sc(100, 200), 127); // clamp
    }
}
