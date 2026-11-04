<#
 Lapis - optional runtime dependencies for Windows.

 Nothing here is bundled with Lapis: each component is downloaded from its official
 publisher when you ask for it.

   -VCRedist   Microsoft Visual C++ Runtime (x64), required by lapis.exe.
               Downloaded from Microsoft; Windows will ask for administrator approval.
   -FFmpeg     ffmpeg.exe, used to import audio/video, change playback speed and convert
               recordings. Downloaded from gyan.dev (release "essentials" build) and copied
               next to lapis.exe. FFmpeg is licensed under the LGPL/GPL (https://ffmpeg.org/legal.html).

 Usage:  powershell -ExecutionPolicy Bypass -File install-deps.ps1 [-VCRedist] [-FFmpeg] [-AppDir <folder>]
#>
param(
    [switch]$VCRedist,
    [switch]$FFmpeg,
    [string]$AppDir = (Split-Path -Parent $MyInvocation.MyCommand.Path)
)
$ErrorActionPreference = "Stop"

$VCUrl     = "https://aka.ms/vs/17/release/vc_redist.x64.exe"
$FFmpegUrl = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip"
$Tmp       = if ($env:TEMP) { $env:TEMP } else { [System.IO.Path]::GetTempPath() }

function Get-File([string]$Url, [string]$Dest) {
    # curl.exe ships with Windows 10 1803+ and follows redirects (aka.ms redirects)
    Write-Host "Downloading $Url"
    & curl.exe -fL --retry 5 --retry-delay 2 --connect-timeout 15 --progress-bar $Url -o $Dest
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path $Dest) -or (Get-Item $Dest).Length -eq 0) {
        throw "Download failed: $Url"
    }
}

function Test-VCRedist {
    $key = "HKLM:\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64"
    return ((Get-ItemProperty $key -ErrorAction SilentlyContinue).Installed -eq 1)
}

function Install-VCRedist {
    if (Test-VCRedist) { Write-Host "Visual C++ Runtime already installed."; return }
    $exe = Join-Path $Tmp "vc_redist.x64.exe"
    Get-File $VCUrl $exe
    # the Microsoft installer needs administrator rights: Windows shows the UAC prompt
    $p = Start-Process -FilePath $exe -ArgumentList "/install", "/passive", "/norestart" -Verb RunAs -Wait -PassThru
    Remove-Item $exe -Force -ErrorAction SilentlyContinue
    # 0 = ok, 3010 = ok (restart suggested), 1638 = a newer version is already installed
    if ($p.ExitCode -notin 0, 3010, 1638) {
        throw "The Visual C++ Runtime installer returned exit code $($p.ExitCode)."
    }
    Write-Host "Visual C++ Runtime installed."
}

function Install-FFmpeg {
    $target = Join-Path $AppDir "ffmpeg.exe"
    if (Test-Path $target) { Write-Host "ffmpeg.exe already present in $AppDir"; return }
    $zip = Join-Path $Tmp "lapis-ffmpeg.zip"
    $dir = Join-Path $Tmp "lapis-ffmpeg"
    Get-File $FFmpegUrl $zip
    if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
    Expand-Archive -LiteralPath $zip -DestinationPath $dir -Force
    $found = Get-ChildItem -Path $dir -Recurse -Filter "ffmpeg.exe" | Select-Object -First 1
    if (-not $found) { throw "ffmpeg.exe not found in the downloaded archive." }
    Copy-Item -LiteralPath $found.FullName -Destination $target -Force
    Remove-Item $zip -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
    Write-Host "ffmpeg.exe installed in $AppDir"
}

try {
    if (-not ($VCRedist -or $FFmpeg)) { Write-Host "Nothing to do. Use -VCRedist and/or -FFmpeg."; exit 0 }
    if ($VCRedist) { Install-VCRedist }
    if ($FFmpeg)   { Install-FFmpeg }
} catch {
    Write-Warning $_.Exception.Message
    Write-Host "You can retry later with:  powershell -ExecutionPolicy Bypass -File `"$PSCommandPath`" -VCRedist -FFmpeg"
    Start-Sleep -Seconds 6
    exit 1
}
