# chords-server-rs

Backend MIDI pour chordJAVA v2 — serveur HTTP Axum qui convertit des requêtes JSON en messages MIDI vers FluidSynth.

## Compilation

```bash
cargo build --release
```

## Dépendances

- Rust edition 2021
- `axum` 0.7 — serveur HTTP
- `midir` 0.9 — sortie MIDI
- `tokio` 1 — async runtime
- `serde` / `serde_json` — JSON
- `tower-http` — CORS

## Routes API

### `GET /`
Page statique (index.html dans `static/`).

### `POST /play`
Lance la lecture d'une séquence d'accords.

**Body :**
```json
{
  "sequence": [{"notes": ["C4","E4","G4","B4"], "beats": 4}],
  "tempo": 120,
  "sig": "4/4",
  "pattern": "rock",
  "walking": false,
  "loop_enabled": false,
  "tracks": [
    {"channel": 0, "program": 51, "volume": 15, "mute": false}
  ]
}
```

### `POST /config`
Configure les paramètres en temps réel.

### `POST /stop`
Arrête la lecture en cours.

## MIDI Tracks

| Index | Canal | Label | Program défaut |
|-------|-------|-------|----------------|
| 0 | 0 | Lead | 51 (Synth Strings) |
| 1 | 2 | Bass | 33 (Acoustic Bass) |
| 2 | 3 | Nappes | 48 (String Ensemble) |
| 3 | 9 | Drums | 1 (Standard Kit) |
| 4 | 4 | Accent | 2 (Bright Acoustic Piano) |

## Fonctionnalités intégrées

- **6 patterns batterie** : Rock, Reggae, Jazz, Pop, Bossa, OneDrop
- **Walking Bass** : chromatique, dominante, diatonique
- **Pompe Skank** : Lead en staccato sur contretemps 8ème
- **Accent 2&4** : Piano Bright Acoustic sur temps 2 et 4
- **A=432Hz** : Pitch bend MIDI sur canaux 0,2,3,4
- **Master Volume** : appliqué à toutes les vélocités
- **Multi-threading** : lecture isolée dans un thread séparé
