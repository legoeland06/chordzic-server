# chords-server-rs — Backend de chordZIC V2

Serveur HTTP **Rust / Axum** qui convertit des requêtes JSON en **MIDI** (FluidSynth /
Roland, lecture temps réel), produit des rendus **WAV** hors-ligne, écoute le
clavier du pianiste (reconnaissance d'accords), relaie l'écho MIDI vers le
périphérique (son de la piste) et sert la synthèse vocale **Piper**.

C'est le moteur de **chordZIC V2** : un séquenceur de grilles d'accords web avec
piano 88 touches illuminé, piano rolls, pistes drums, boucles d'échantillons et
export audio.

- Frontend : [chordzic-frontend](https://github.com/legoeland06/chordzic-frontend)
- Écoute sur **http://localhost:4000** par défaut.

---

## Aperçu

En mode **standalone**, le serveur embarque le frontend complet :

![Grille d'accords en mode Live](screenshots/grille_accords_texte_modeLive.png)
*Grille d'accords en saisie texte (mode Live)*

---

## Fonctionnalités

- **Lecture MIDI temps réel** multi-pistes : Lead, Bass, Nappes, Drums, Accent
  (6 patterns batterie, walking bass, pompe skank, accent 2&4, A=432 Hz, master volume)
- **🎹 Reconnaissance d'accords (mode Live)** : `GET /live-input` relaie les notes
  tenues sur le clavier du pianiste (Roland) — ordre d'arrivée **conservé** (l'ordre
  d'appui du pianiste est transmis tel quel, la reco trie elle-même ses classes)
- **🎛 Écho MIDI / son de la piste (mode Navig)** : `POST /live-echo` — avec une piste
  sélectionnée, le serveur envoie au périphérique le **program change** de la piste
  (banque + PC, canal drums natif) et renvoie les notes du pianiste sur son canal
  (il sonne avec l'instrument de la piste) ; **pédale de sustain (CC64)** relayée et
  état mémorisé (relancé à l'activation)
- **🔴 Rec MIDI (mode Navig)** : `POST /rec-midi` / `GET /rec-midi-state` — enregistrement
  du clavier du pianiste en événements **horodatés** (ordre d'appui conservé, repiquage
  ignoré), restitués au frontend pour insertion dans le piano roll
- **🗣 Synthèse vocale** : `POST /tts` — proxy vers le serveur **Piper** local
  (env `PIPER_URL`, défaut `http://127.0.0.1:5001/synthesize`) pour la lecture des
  rubriques d'aide à voix haute (WAV renvoyé, même origine → pas de CORS)
- **Rendu WAV hors-ligne** : morceau complet (`/render-wav`, grille **et/ou** notes
  personnalisées — une grille vide avec des notes se lit), par pistes, notes isolées,
  et rendu externe (enregistrement du périphérique)
- **Silences réels** : `4:_`, `2:_`, `1:_` coupent tout mais le timing avance
- **Échantillons (samples)** : liste, fichier, boucle alignée sur la grille
- **Grilles** : sauvegarde / liste / suppression côté serveur (JSON)
- **Métronome** : click paramétrable, sortie dédiée (double canaux), démarrage/arrêt
  en mode navigation (`/navig-*`)
- **Frontend embarqué** (feature `standalone`) : le serveur sert l'application React complète

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

⚠️ **MIDI** : auto-détection du port par nom (priorité **Roland**, puis FluidSynth,
puis premier port utilisable). Pour forcer un index : `MIDI_PORT=N ./chords-server-rs`.
Ne pas définir `MIDI_PORT` sur un index invalide → serveur muet.
⚠️ **TTS** : la synthèse vocale de l'aide nécessite un serveur Piper local (ou
`PIPER_URL` pointant ailleurs).

## Routes API

| Méthode | Route | Description |
|---------|-------|-------------|
| GET | `/` | Page (frontend embarqué en standalone, sinon `static/index.html`) |
| POST | `/play` | Lance la lecture d'une séquence d'accords |
| POST | `/config` | Paramètres en temps réel |
| POST | `/stop` | Arrête la lecture |
| POST | `/note` | Note MIDI immédiate (canal, note, vélocité, durée) |
| POST | `/render-wav` | Rendu WAV du morceau (grille + notes personnalisées) |
| POST | `/render-external` | Rendu WAV via le périphérique MIDI (temps réel, enregistré) |
| POST | `/render-tracks` | Rendu WAV par piste (bounce PostProd) |
| POST | `/render-notes` | Notes isolées (pré-remplissage piano roll) |
| GET | `/live-input` | Notes tenues sur le clavier du pianiste (ordre d'arrivée) |
| POST | `/live-echo` | Écho MIDI + program change de la piste sélectionnée |
| POST | `/rec-midi` · GET `/rec-midi-state` | Enregistrement MIDI horodaté (mode Navig) |
| POST | `/tts` | Synthèse vocale Piper (proxy, renvoie un WAV) |
| GET | `/samples-list` · `/sample-file/:name` | Échantillons |
| GET | `/rendered/:file` | Fichier de rendu généré |
| GET/POST | `/audio-devices` / `/audio-device` | Périphériques audio |
| GET/POST | `/midi-ports` / `/midi-port` | Ports MIDI (sortie) |
| GET/POST | `/click` | Configuration / état du métronome |
| POST | `/navig-click-start` / `/navig-click-stop` | Métronome en mode navigation |
| POST | `/navig-play` | Lecture navigation (double canaux) |
| POST | `/navig-play-midi` / `/navig-stop-midi` | Lecture MIDI de toutes les pistes (`exclude_channel` = play-along REC : la piste enregistrée ne joue pas) |
| POST | `/save` · GET `/grilles` · DELETE `/grilles/:name` | Persistance des grilles |

## Tracks MIDI par défaut

| Index | Canal | Label | Program par défaut |
|-------|-------|-------|--------------------|
| 0 | 0 | Lead | 51 (Synth Strings) |
| 1 | 2 | Bass | 33 (Acoustic Bass) |
| 2 | 3 | Nappes | 48 (String Ensemble) |
| 3 | 9 | Drums | 1 (Standard Kit) |
| 4 | 4 | Accent | 2 (Bright Acoustic Piano) |

## Modules sources

- `main.rs` — serveur Axum, routes, orchestration, TTS proxy
- `midi.rs` — sortie MIDI (midir), FluidSynth
- `live_input.rs` — écoute du clavier (notes tenues, ordre d'arrivée), écho MIDI,
  pédale de sustain, program change, **session d'enregistrement MIDI** (RecSession)
- `patterns.rs` — patterns batterie
- `walking.rs` — walking bass
- `render.rs` — rendu WAV hors-ligne (hound + synthèse)
- `external_render.rs` — rendu externe (périphérique MIDI)
- `samples.rs` — échantillons, recadrage sur la grille
- `click.rs` — métronome
- `dsp.rs` — traitement audio
- `grilles.rs` — persistance des grilles

## Dépendances principales

`axum` 0.7 · `tokio` 1 · `serde`/`serde_json` · `tower-http` (CORS) · `midir` 0.9 ·
`rodio` 0.18 · `cpal` 0.15 · `hound` 3.5 · `ureq` (proxy TTS) · `rust-embed` (standalone) ·
`mime_guess`

## Tests

```bash
cargo test --release
```

76 tests : parseur d'accords, patterns, walking bass, ticks SMF, entrée live
(ordre d'arrivée), écho MIDI, program change, enregistrement MIDI, TTS
(serveur Piper factice).

---

## Développement

**Eric BRUNEAU** — vibe coding Deepseek (legoeland)
