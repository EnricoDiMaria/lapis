<#
 Lapis - script di setup Windows (equivalente di install.sh)
 Uso:  powershell -ExecutionPolicy Bypass -File install.ps1 [install|local|update|model|ffmpeg|uninstall|help] [-Archive .\lapis-windows-x86_64.zip]
#>
param(
    [Parameter(Position = 0)][string]$Command = "install",
    [string]$Archive = ".\lapis-windows-x86_64.zip"
)
$ErrorActionPreference = "Stop"

$Repo       = "EnricoDiMaria/lapis"
$ReleaseUrl = "https://github.com/$Repo/releases/latest/download/lapis-windows-x86_64.zip"
$AppDir     = Join-Path $env:LOCALAPPDATA "Programs\Lapis"
$DataDir    = Join-Path $env:LOCALAPPDATA "lapis"          # = paths::data_dir()
$CfgDir     = Join-Path $env:APPDATA "lapis"               # = paths::config_dir()
$ModelDir   = Join-Path $DataDir "models\parakeet-tdt-0.6b-v3-int8"
$ModelBase  = "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main"
$StartMenu  = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Lapis.lnk"

function Download($url, $dest) {
    # curl.exe è incluso in Windows 10 1803+: stesso comportamento di install.sh (resume + retry)
    Write-Host "   $(Split-Path $dest -Leaf)"
    & curl.exe -fL --retry 5 --retry-delay 2 --connect-timeout 15 -C - --progress-bar $url -o "$dest.part"
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path "$dest.part") -or (Get-Item "$dest.part").Length -eq 0) {
        Remove-Item "$dest.part" -ErrorAction SilentlyContinue
        throw "Download fallito: $url"
    }
    Move-Item -Force "$dest.part" $dest
}

function Install-Binary([bool]$FromLocal) {
    New-Item -ItemType Directory -Force $AppDir | Out-Null
    $zip = $Archive
    if (-not $FromLocal -and -not (Test-Path $Archive)) {
        $zip = Join-Path $env:TEMP "lapis-windows-x86_64.zip"
        Write-Host "-> Download di Lapis da GitHub..."
        Download $ReleaseUrl $zip
    } elseif (Test-Path $zip) {
        Write-Host "-> Installazione da archivio locale: $zip"
    } elseif (Test-Path ".\target\release\lapis.exe") {
        Copy-Item -Force ".\target\release\lapis.exe" (Join-Path $AppDir "lapis.exe")
        $zip = $null
    } else { throw "Né '$Archive' né .\target\release\lapis.exe trovati." }

    if ($zip) { Expand-Archive -Force -Path $zip -DestinationPath $AppDir }
    if (-not (Test-Path (Join-Path $AppDir "lapis.exe"))) { throw "lapis.exe non trovato dopo l'estrazione." }
    Write-Host "OK Binario installato in $AppDir"
}

function Install-Shortcut {
    New-Item -ItemType Directory -Force (Split-Path $StartMenu) | Out-Null
    $sh = New-Object -ComObject WScript.Shell
    $lnk = $sh.CreateShortcut($StartMenu)
    $lnk.TargetPath = Join-Path $AppDir "lapis.exe"
    $lnk.WorkingDirectory = $AppDir
    $lnk.Description = "Advanced PDF study suite"
    $lnk.Save()
    # "Apri con" per i PDF (per-utente, nessun admin)
    $k = "HKCU:\Software\Classes\Applications\lapis.exe"
    New-Item -Force "$k\shell\open\command" | Out-Null
    Set-ItemProperty "$k\shell\open\command" "(default)" "`"$AppDir\lapis.exe`" `"%1`""
    New-Item -Force "$k\SupportedTypes" | Out-Null
    Set-ItemProperty "$k\SupportedTypes" ".pdf" ""
    Write-Host "OK Collegamento nel menu Start e voce 'Apri con' registrati"
}

function Install-Model {
    Write-Host "-> Download modello Parakeet TDT 0.6B v3 (int8) (~650 MB). I download interrotti riprendono rilanciando."
    New-Item -ItemType Directory -Force $ModelDir | Out-Null
    foreach ($f in "vocab.txt", "encoder-model.int8.onnx", "decoder_joint-model.int8.onnx") {
        $dest = Join-Path $ModelDir $f
        if ((Test-Path $dest) -and (Get-Item $dest).Length -gt 0 -and -not (Test-Path "$dest.part")) {
            Write-Host "   $f già presente, salto"; continue
        }
        Download "$ModelBase/$f" $dest
    }
    Write-Host "OK Modello installato in $ModelDir"
}

function Install-Ffmpeg {
    if (Get-Command ffmpeg -ErrorAction SilentlyContinue) { Write-Host "ffmpeg già nel PATH"; return }
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        winget install --id Gyan.FFmpeg -e --accept-source-agreements --accept-package-agreements
        Write-Host "OK ffmpeg installato: riavvia la sessione (o fai logout/login) perché Lapis veda il nuovo PATH."
        Write-Host "   In alternativa copia ffmpeg.exe in $AppDir (viene trovato automaticamente)."
    } else { Write-Host "winget non disponibile: copia ffmpeg.exe in $AppDir" }
}

function Uninstall-Lapis {
    Remove-Item -Recurse -Force $AppDir -ErrorAction SilentlyContinue
    Remove-Item -Force $StartMenu -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force "HKCU:\Software\Classes\Applications\lapis.exe" -ErrorAction SilentlyContinue
    Write-Host "OK Lapis rimosso."
    $a = Read-Host "Rimuovere anche modelli/cache/impostazioni ($DataDir, $CfgDir)? [y/N]"
    if ($a -match '^[Yy]$') { Remove-Item -Recurse -Force $DataDir, $CfgDir -ErrorAction SilentlyContinue; Write-Host "OK Dati rimossi" }
}

switch -Regex ($Command) {
    '^(install|--install)$' {
        Install-Binary $false; Install-Shortcut
        if ((Read-Host "Scaricare anche il modello ASR (~650 MB)? [y/N]") -match '^[Yy]$') { Install-Model }
        if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue) -and -not (Test-Path (Join-Path $AppDir "ffmpeg.exe"))) {
            if ((Read-Host "ffmpeg non trovato (serve per import media / velocità audio). Installarlo con winget? [y/N]") -match '^[Yy]$') { Install-Ffmpeg }
        }
        Write-Host "OK Lapis installato."
    }
    '^(local|--local)$'   { Install-Binary $true; Install-Shortcut }
    '^(update|--update)$' { Install-Binary $false }
    '^(model|--model)$'   { Install-Model }
    '^ffmpeg$'            { Install-Ffmpeg }
    '^(uninstall|--uninstall)$' { Uninstall-Lapis }
    default { Get-Help $MyInvocation.MyCommand.Path | Out-Null; Write-Host "Comandi: install | local | update | model | ffmpeg | uninstall" }
}
