# ═══════════════════════════════════════════════════════════════════════════════
# install_windows.ps1 — Installation de chordZIC v2 sur Windows (10 / 11)
# v2026-08-20.1
# ═══════════════════════════════════════════════════════════════════════════════
#
# Compile le binaire STANDALONE (backend Rust + frontend React embarqués) et
# installe les dépendances : Rust, Git, FluidSynth (exe + DLL), SoundFont.
#
# Usage (PowerShell, en tant qu'utilisateur normal) :
#   powershell -ExecutionPolicy Bypass -File install_windows.ps1
#
# Résultat final :
#   %USERPROFILE%\chordzic\
#     chords-server-rs.exe  → Binaire unique (backend + frontend)
#     fluidsynth.exe + DLLs → Moteur de synthèse (rendu WAV / MIDI)
#     MuseScore_General_Full.sf3 → SoundFont
#     chordzic.bat          → Lanceur (démarre le serveur + ouvre le navigateur)
#     chordzic-stop.bat     → Arrête serveur + FluidSynth
#
# Le serveur écoute sur http://localhost:4000 (le navigateur affiche l'app).
#
# ═══════════════════════════════════════════════════════════════════════════════

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$REPO_URL        = "https://github.com/legoeland06/chordzic-server.git"
$FLUID_URL       = "https://github.com/FluidSynth/fluidsynth/releases/download/v2.6.0/fluidsynth-v2.6.0-win10-x64-cpp11.zip"
$SF_URL          = "https://ftp.osuosl.org/pub/musescore/soundfont/MuseScore_General/MuseScore_General_Full.sf3"

$INSTALL_DIR     = Join-Path $HOME "chordzic"
$SRC_DIR         = Join-Path $HOME "chordzic-src"
$TMP_DIR         = Join-Path $env:TEMP "chordzic-setup"

function Info  { Write-Host "  ℹ️  $args" -ForegroundColor Cyan }
function Ok    { Write-Host "  ✅ $args" -ForegroundColor Green }
function Warn  { Write-Host "  ⚠️  $args" -ForegroundColor Yellow }
function Err   { Write-Host "  ❌ $args" -ForegroundColor Red; exit 1 }
function Step  { Write-Host "`n━━━ $args ━━━" -ForegroundColor Blue }

Write-Host ""
Write-Host "╔══════════════════════════════════════════╗" -ForegroundColor Cyan
Write-Host "║   🎵 chordZIC v2 — Installation Windows  ║" -ForegroundColor Cyan
Write-Host "╚══════════════════════════════════════════╝" -ForegroundColor Cyan
Info "Cible : $INSTALL_DIR"

New-Item -ItemType Directory -Force -Path $TMP_DIR | Out-Null

# ── 1/6 — Git ────────────────────────────────────────────────────────────────
Step "1/6 — Git"
if (Get-Command git -ErrorAction SilentlyContinue) {
    Ok "Git déjà installé ($(git --version))"
} else {
    Info "Installation de Git (winget)..."
    winget install --id Git.Git -e --accept-package-agreements --accept-source-agreements | Out-Null
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) { Err "Git introuvable après installation — réessaie après redémarrage du terminal." }
    Ok "Git installé"
}

# ── 2/6 — Rust (rustup + toolchain MSVC) ─────────────────────────────────────
Step "2/6 — Rust"
$cargoBin = Join-Path $HOME ".cargo\bin"
$env:Path = "$cargoBin;$env:Path"
if (Get-Command rustc -ErrorAction SilentlyContinue) {
    Ok "Rust déjà installé ($(rustc --version))"
} else {
    Info "Téléchargement de rustup-init..."
    $rustup = Join-Path $TMP_DIR "rustup-init.exe"
    curl.exe -L -o $rustup "https://win.rustup.rs/x86_64" 2>$null
    if (-not (Test-Path $rustup)) { Err "Échec du téléchargement de rustup" }
    Info "Installation silencieuse de rustup + toolchain MSVC (quelques minutes)..."
    & $rustup -y --default-toolchain stable | Out-Null
    $env:Path = "$cargoBin;$env:Path"
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { Err "cargo introuvable après installation — relance le script dans un nouveau terminal." }
    Ok "Rust installé ($(rustc --version))"
}

# MSVC Build Tools (nécessaires au linking du target par défaut)
Step "3/6 — Build Tools Visual Studio (C++)"
$vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vswhereFound = Test-Path $vsWhere
$vcFound = $false
if ($vswhereFound) {
    $vcFound = & $vsWhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($vcFound) { Ok "Build Tools C++ déjà installés" }
}
if (-not $vcFound) {
    Warn "Les Build Tools Visual Studio (C++) sont requis pour compiler — gros téléchargement (~3-6 Go), une seule fois."
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        Info "Installation via winget (peut prendre 10-20 min)..."
        winget install --id Microsoft.VisualStudio.2022.BuildTools -e --accept-package-agreements --accept-source-agreements `
            --override "--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended" | Out-Null
    } else {
        Info "Téléchargement de vs_BuildTools.exe..."
        $vbt = Join-Path $TMP_DIR "vs_BuildTools.exe"
        curl.exe -L -o $vbt "https://aka.ms/vs/17/release/vs_BuildTools.exe" 2>$null
        Info "Installation (interface graphique — coche « Développement Desktop en C++ »)..."
        & $vbt --passive --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended
    }
    # Re-évaluation
    if ($vswhereFound) { $vcFound = & $vsWhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath }
    if (-not $vcFound) { Warn "Build Tools non détectés — la compilation échouera au linking. Réinstalle-les puis relance." }
}

# ── 4/6 — FluidSynth (exe + DLL, sortie audio Windows) ───────────────────────
Step "4/6 — FluidSynth"
$fluidZip = Join-Path $TMP_DIR "fluidsynth.zip"
if (-not (Test-Path (Join-Path $INSTALL_DIR "fluidsynth.exe"))) {
    Info "Téléchargement de FluidSynth 2.6.0 (win64)..."
    curl.exe -L -o $fluidZip $FLUID_URL 2>$null
    if (-not (Test-Path $fluidZip)) { Err "Échec du téléchargement de FluidSynth" }
    Expand-Archive -Path $fluidZip -DestinationPath $TMP_DIR -Force
    New-Item -ItemType Directory -Force -Path $INSTALL_DIR | Out-Null
    Copy-Item (Join-Path $TMP_DIR "fluidsynth-*\bin\*") $INSTALL_DIR -Recurse -Force
    Ok "FluidSynth installé dans $INSTALL_DIR"
} else {
    Ok "FluidSynth déjà présent"
}

# ── 5/6 — SoundFont MuseScore General Full ────────────────────────────────────
Step "5/6 — SoundFont (~82 Mo)"
$sfPath = Join-Path $INSTALL_DIR "MuseScore_General_Full.sf3"
if (-not (Test-Path $sfPath)) {
    Info "Téléchargement depuis musescore.org..."
    curl.exe -L -o $sfPath $SF_URL 2>$null
    if (-not (Test-Path $sfPath)) { Err "Échec du téléchargement de la SoundFont" }
    Ok "SoundFont téléchargée ($([math]::Round((Get-Item $sfPath).Length / 1MB)) Mo)"
} else {
    Ok "SoundFont déjà présente"
}

# ── 6/6 — Code source + compilation ──────────────────────────────────────────
Step "6/6 — Compilation du binaire standalone"
$serverDir = Join-Path $SRC_DIR "server-rs"
if (-not (Test-Path (Join-Path $serverDir "Cargo.toml"))) {
    New-Item -ItemType Directory -Force -Path $SRC_DIR | Out-Null
    Info "Clone du dépôt chordzic-server (frontend embarqué inclus)..."
    git clone --depth 1 $REPO_URL $SRC_DIR
} else {
    Info "Dépôt présent — mise à jour..."
    Push-Location $SRC_DIR
    git pull
    Pop-Location
}
if (-not (Test-Path (Join-Path $serverDir "Cargo.toml"))) { Err "Dépôt cloné sans server-rs ?" }

Push-Location $serverDir
Info "cargo build --release --features standalone (5-15 min la première fois)..."
cargo build --release --features standalone
if ($LASTEXITCODE -ne 0) { Err "La compilation a échoué" }
Pop-Location

$bin = Join-Path $serverDir "target\release\chords-server-rs.exe"
if (-not (Test-Path $bin)) { Err "Binaire introuvable après compilation" }
Copy-Item $bin $INSTALL_DIR -Force
Ok "Binaire copié : $INSTALL_DIR\chords-server-rs.exe"

# ── Lanceurs ─────────────────────────────────────────────────────────────────
$bat = Join-Path $INSTALL_DIR "chordzic.bat"
@"
@echo off
cd /d "%~dp0"
start "" /b chords-server-rs.exe
timeout /t 2 /nobreak >nul
start "" http://localhost:4000/
"@ | Set-Content -Path $bat -Encoding ASCII

$batStop = Join-Path $INSTALL_DIR "chordzic-stop.bat"
@"
@echo off
taskkill /IM chords-server-rs.exe /F >nul 2>&1
taskkill /IM fluidsynth.exe /F >nul 2>&1
echo chordZIC arrete.
"@ | Set-Content -Path $batStop -Encoding ASCII

Write-Host ""
Write-Host "🎵 chordZIC v2 installé !" -ForegroundColor Green
Write-Host "   Lance :  $bat" -ForegroundColor White
Write-Host "   Arrêt :  $batStop" -ForegroundColor White
Write-Host "   (le serveur écoute sur http://localhost:4000)" -ForegroundColor Gray
