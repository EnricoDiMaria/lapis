<#
 Lapis - Windows setup script (equivalent of install.sh)
 Usage:  powershell -ExecutionPolicy Bypass -File install.ps1 [install|local|update|model|ffmpeg|uninstall|help] [-Archive .\lapis-windows-x86_64.zip]
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
    # curl.exe ships with Windows 10 1803+: same behaviour as install.sh (resume + retry)
    Write-Host "   $(Split-Path $dest -Leaf)"
    & curl.exe -fL --retry 5 --retry-delay 2 --connect-timeout 15 -C - --progress-bar $url -o "$dest.part"
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path "$dest.part") -or (Get-Item "$dest.part").Length -eq 0) {
        Remove-Item "$dest.part" -ErrorAction SilentlyContinue
        throw "Download failed: $url"
    }
    Move-Item -Force "$dest.part" $dest
}

function Install-Binary([bool]$FromLocal) {
    New-Item -ItemType Directory -Force $AppDir | Out-Null
    $zip = $Archive
    if (-not $FromLocal -and -not (Test-Path $Archive)) {
        $zip = Join-Path $env:TEMP "lapis-windows-x86_64.zip"
        Write-Host "-> Downloading Lapis from GitHub..."
        Download $ReleaseUrl $zip
    } elseif (Test-Path $zip) {
        Write-Host "-> Installing from local archive: $zip"
    } elseif (Test-Path ".\target\release\lapis.exe") {
        Copy-Item -Force ".\target\release\lapis.exe" (Join-Path $AppDir "lapis.exe")
        $zip = $null
    } else { throw "Neither '$Archive' nor .\target\release\lapis.exe was found." }

    if ($zip) { Expand-Archive -Force -Path $zip -DestinationPath $AppDir }
    if (-not (Test-Path (Join-Path $AppDir "lapis.exe"))) { throw "lapis.exe not found after extraction." }
    Write-Host "OK Binary installed in $AppDir"
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
    Write-Host "OK Start menu shortcut and 'Open with' entry registered"
}

function Install-Model {
    Write-Host "-> Downloading the Parakeet TDT 0.6B v3 (int8) model (~650 MB). Interrupted downloads resume when you run this again."
    New-Item -ItemType Directory -Force $ModelDir | Out-Null
    foreach ($f in "vocab.txt", "encoder-model.int8.onnx", "decoder_joint-model.int8.onnx") {
        $dest = Join-Path $ModelDir $f
        if ((Test-Path $dest) -and (Get-Item $dest).Length -gt 0 -and -not (Test-Path "$dest.part")) {
            Write-Host "   $f already present, skipping"; continue
        }
        Download "$ModelBase/$f" $dest
    }
    Write-Host "OK Model installed in $ModelDir"
}

function Install-Ffmpeg {
    if (Get-Command ffmpeg -ErrorAction SilentlyContinue) { Write-Host "ffmpeg is already in PATH"; return }
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        winget install --id Gyan.FFmpeg -e --accept-source-agreements --accept-package-agreements
        Write-Host "OK ffmpeg installed: restart your session (or log out and in) so Lapis sees the new PATH."
        Write-Host "   Alternatively copy ffmpeg.exe into $AppDir (it is found automatically)."
    } else { Write-Host "winget is not available: copy ffmpeg.exe into $AppDir" }
}

function Uninstall-Lapis {
    Remove-Item -Recurse -Force $AppDir -ErrorAction SilentlyContinue
    Remove-Item -Force $StartMenu -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force "HKCU:\Software\Classes\Applications\lapis.exe" -ErrorAction SilentlyContinue
    Write-Host "OK Lapis removed."
    $a = Read-Host "Also remove ASR models, audio recordings, notes backups, dictionaries and settings ($DataDir, $CfgDir)? Notes saved inside your PDFs are not affected. [y/N]"
    if ($a -match '^[Yy]$') { Remove-Item -Recurse -Force $DataDir, $CfgDir -ErrorAction SilentlyContinue; Write-Host "OK Data removed" }
}

switch -Regex ($Command) {
    '^(install|--install)$' {
        Install-Binary $false; Install-Shortcut
        if ((Read-Host "Also download the ASR model (~650 MB)? [y/N]") -match '^[Yy]$') { Install-Model }
        if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue) -and -not (Test-Path (Join-Path $AppDir "ffmpeg.exe"))) {
            if ((Read-Host "ffmpeg not found (needed to import media and change audio speed). Install it with winget? [y/N]") -match '^[Yy]$') { Install-Ffmpeg }
        }
        Write-Host "OK Lapis installed."
    }
    '^(local|--local)$'   { Install-Binary $true; Install-Shortcut }
    '^(update|--update)$' { Install-Binary $false }
    '^(model|--model)$'   { Install-Model }
    '^ffmpeg$'            { Install-Ffmpeg }
    '^(uninstall|--uninstall)$' { Uninstall-Lapis }
    default { Get-Help $MyInvocation.MyCommand.Path | Out-Null; Write-Host "Commands: install | local | update | model | ffmpeg | uninstall" }
}
