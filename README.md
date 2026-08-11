# chords-server-rs — Backend de chordZIC V2

Serveur HTTP **Rust / Axum** qui convertit des requêtes JSON en messages **MIDI** vers
**FluidSynth** (lecture temps réel) et produit des rendus **WAV** hors-ligne.

C'est le moteur de **chordZIC V2** : un séquenceur de grilles d'accords web avec
piano rolls, pistes drums, boucles d'échantillons et export audio.

- Frontend : [chordzic-frontend](https://github.com/legoeland06/chordzic-frontend)
- Écoute sur **http://localhost:4000** par défaut.

---

## Aperçu

En mode **standalone**, le serveur embarque le frontend complet :

![Liste des pistes en mode Live](screenshots/controlPanel_trackListe_modeLive.png)
*Contrôle et liste des pistes (mode Live — MIDI vers FluidSynth)*

![Grille d'accords en mode Live](screenshots/grille_accords_texte_modeLive.png)
*Grille d'accords en saisie texte (mode Live)*

---

## Fonctionnalités

- **Lecture MIDI temps réel** multi-pistes : Lead, Bass, Nappes, Drums, Accent
- **6 patterns batterie** : Rock, Reggae, Jazz, Pop, Bossa, OneDrop
- **Walking Bass** : chromatique, dominante, diatonique
- **Pompe skank** : Lead en staccato sur les contretemps 8ème
- **Accent 2&4** : piano Bright Acoustic sur les temps 2 et 4 (désactivé sur les accords courts)
- **A = 432 Hz** : pitch bend MIDI sur les canaux mélodiques
- **Master volume** : appliqué à toutes les vélocités
- **Pistes drums** : les notes des pistes drums sont jouées sur le canal 9 (kit GM standard)
- **Silences réels** : `4:_`, `2:_`, `1:_` coupent tout (accords, nappes, drums) mais le timing avance
- **Rendu WAV hors-ligne** : morceau complet, par pistes, ou notes isolées
- **Échantillons (samples)** : liste, fichier, boucle alignée sur la grille (recadrage tempo/signature côté extraction)
- **Grilles** : sauvegarde / liste / suppression côté serveur (JSON)
- **Métronome** : click paramétrable + démarrage/arrêt en mode navigation
- **Frontend embarqué** (feature `standalone`) : le serveur sert l'application React complète

---

## Compilation

```bash
# Mode développement (sert static/index.html sur /)
cargo build

# Binaire standalone avec le frontend React embarqué (recommandé)
cargo build --release --features standalone
```

Prérequis : Rust ≥ 1.97 (edition 2021), FluidSynth installé avec une SoundFont
(ex. `MuseScore_General_Full.sf3`).

## Lancement

```bash
# Depuis le dossier du binaire
./chords-server-rs
# puis ouvrir http://localhost:4000
```

⚠️ **MIDI** : par défaut, le serveur **auto-détecte** le port FluidSynth (mode
recommandé). Pour forcer un index : `MIDI_PORT=N ./chords-server-rs`.
Ne pas définir `MIDI_PORT` sur un index invalide → serveur muet.

## Routes API

| Méthode | Route | Description |
|---------|-------|-------------|
| GET | `/` | Page (frontend embarqué en standalone, sinon `static/index.html`) |
| POST | `/play` | Lance la lecture d'une séquence d'accords (tempo, signature, pattern, tracks…) |
| POST | `/config` | Paramètres en temps réel |
| POST | `/stop` | Arrête la lecture |
| POST | `/note` | Note MIDI immédiate (canal, note, vélocité) |
| POST | `/render-wav` | Rendu WAV du morceau complet |
| POST | `/render-tracks` | Rendu WAV par piste |
| POST | `/render-notes` | Rendu WAV de notes isolées |
| GET | `/samples-list` | Liste des échantillons disponibles |
| GET | `/sample-file/:name` | Fichier d'un échantillon |
| GET | `/rendered/:file` | Fichier de rendu généré |
| GET | `/audio-devices` | Périphériques audio disponibles |
| GET/POST | `/click` | Configuration / état du métronome |
| POST | `/navig-click-start` / `/navig-click-stop` | Métronome en mode navigation |
| POST | `/navig-play` | Lecture en mode navigation |
| POST | `/save` | Sauvegarde une grille |
| GET | `/grilles` | Liste des grilles sauvegardées |
| DELETE | `/grilles/:name` | Supprime une grille |

## Tracks MIDI par défaut

| Index | Canal | Label | Program par défaut |
|-------|-------|-------|--------------------|
| 0 | 0 | Lead | 51 (Synth Strings) |
| 1 | 2 | Bass | 33 (Acoustic Bass) |
| 2 | 3 | Nappes | 48 (String Ensemble) |
| 3 | 9 | Drums | 1 (Standard Kit) |
| 4 | 4 | Accent | 2 (Bright Acoustic Piano) |

## Modules sources

- `main.rs` — serveur Axum, routes, orchestration
- `midi.rs` — sortie MIDI (midir), FluidSynth
- `patterns.rs` — patterns batterie
- `walking.rs` — walking bass
- `render.rs` — rendu WAV hors-ligne (hound + synthèse)
- `samples.rs` — échantillons, recadrage sur la grille
- `click.rs` — métronome
- `dsp.rs` — traitement audio
- `grilles.rs` — persistance des grilles

## Dépendances principales

`axum` 0.7 · `tokio` 1 · `serde`/`serde_json` · `tower-http` (CORS) · `midir` 0.9 ·
`rodio` 0.18 · `cpal` 0.15 · `hound` 3.5 · `rust-embed` (standalone) · `mime_guess`

---

## Développement

**Eric BRUNEAU** — vibe coding Deepseek (legoeland)
