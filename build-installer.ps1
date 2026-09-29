# Construit dist\Wayne-Setup-<version>.exe (installateur distribuable, un seul fichier).
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not (Test-Path assets\wayne.ico)) { & .\assets\make-icon.ps1 }

# cargo écrit sa progression sur stderr : on juge sur le code de sortie, pas sur stderr.
function Run([string]$exe, [string[]]$argv) {
    $ErrorActionPreference = 'Continue'
    & $exe @argv 2>&1 | ForEach-Object { "$_" }
    if ($LASTEXITCODE -ne 0) { throw "échec : $exe $argv" }
}

Run cargo @('build', '--release', '-p', 'stamp')
Run cargo @('build', '--release', '-p', 'wayne')
Run target\release\stamp.exe @('target\release\wayne.exe', 'assets\wayne.ico', $version, 'Wayne, lanceur d''applications', 'wayne.exe')

# L'installateur embarque wayne.exe (déjà estampillé) : on force sa recompilation.
(Get-Item installer\src\main.rs).LastWriteTime = Get-Date
Run cargo @('build', '--release', '-p', 'wayne-setup')
Run target\release\stamp.exe @('target\release\wayne-setup.exe', 'assets\wayne.ico', $version, 'Installateur de Wayne', 'Wayne-Setup.exe')

New-Item -ItemType Directory -Force dist | Out-Null
$out = "dist\Wayne-Setup-$version.exe"
Copy-Item target\release\wayne-setup.exe $out -Force
$hash = (Get-FileHash $out -Algorithm SHA256).Hash
"$hash  Wayne-Setup-$version.exe" | Set-Content "$out.sha256" -Encoding ascii
Write-Host "OK : $out ($([math]::Round((Get-Item $out).Length / 1KB)) Ko)  SHA-256 $hash"
