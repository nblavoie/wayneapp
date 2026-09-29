# Prépare site-out\ : le site statique prêt à téléverser sur ton hébergeur.
#   powershell -ExecutionPolicy Bypass -File build-site.ps1 -Url https://ton-domaine.com
# Lance d'abord build-installer.ps1 pour que dist\ contienne l'installateur à jour.
param([string]$Url = "https://example.com")
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
Add-Type -AssemblyName System.Drawing

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$setup = "dist\Wayne-Setup-$version.exe"
if (-not (Test-Path $setup)) { throw "$setup introuvable : lance d'abord build-installer.ps1" }

$out = "site-out"
if (Test-Path $out) { Remove-Item $out -Recurse -Force }
New-Item -ItemType Directory -Force "$out\download" | Out-Null

# Installateur : nom stable (lien de la page) + nom versionné.
Copy-Item $setup "$out\download\Wayne-Setup.exe"
Copy-Item $setup "$out\download\Wayne-Setup-$version.exe"
$sha = (Get-FileHash $setup -Algorithm SHA256).Hash
"$sha  Wayne-Setup-$version.exe" | Set-Content "$out\download\Wayne-Setup-$version.exe.sha256" -Encoding ascii
$sizeNum = [math]::Round((Get-Item $setup).Length / 1KB)
$size = "$sizeNum Ko"

# Page : remplacement des variables.
$u8 = New-Object Text.UTF8Encoding $false
$html = [IO.File]::ReadAllText((Resolve-Path site\index.html), [Text.Encoding]::UTF8)
$html = $html.Replace('{{SIZE_NUM}}', $sizeNum).Replace('{{SIZE_UNIT}}', 'Ko').Replace('{{VERSION}}', $version).Replace('{{SIZE}}', $size).Replace('{{SHA256}}', $sha).Replace('{{SITE_URL}}', $Url.TrimEnd('/')).Replace('{{YEAR}}', (Get-Date).Year)
[IO.File]::WriteAllText("$PWD\$out\index.html", $html, $u8)
Copy-Item assets\wayne.ico "$out\wayne.ico"
Copy-Item site\kortexs-logo.svg "$out\kortexs-logo.svg"

function New-Canvas($w, $h) { $b = New-Object System.Drawing.Bitmap $w, $h; $g = [System.Drawing.Graphics]::FromImage($b); $g.SmoothingMode = 'AntiAlias'; $g.TextRenderingHint = 'AntiAliasGridFit'; $g.PixelOffsetMode = 'Half'; return $b, $g }
function Fill-Round($g, $brush, $x, $y, $w, $h, $r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath; $d = 2 * $r
    $p.AddArc($x, $y, $d, $d, 180, 90); $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90); $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90); $p.CloseFigure(); $g.FillPath($brush, $p)
}
$teal = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 0x35, 0x7F, 0x87))
$center = New-Object System.Drawing.StringFormat; $center.Alignment = 'Center'; $center.LineAlignment = 'Center'

# icon.png (512 px)
$b, $g = New-Canvas 512 512
Fill-Round $g $teal 16 16 480 480 124
$g.DrawString('W', (New-Object System.Drawing.Font 'Segoe UI', 290, ([System.Drawing.FontStyle]::Bold), ([System.Drawing.GraphicsUnit]::Pixel)), [System.Drawing.Brushes]::White, (New-Object System.Drawing.RectangleF 0, 10, 512, 512), $center)
$b.Save("$PWD\$out\icon.png", [System.Drawing.Imaging.ImageFormat]::Png); $g.Dispose(); $b.Dispose()

# og.png (1200 × 630) pour les aperçus de liens
$b, $g = New-Canvas 1200 630
$g.Clear([System.Drawing.Color]::FromArgb(255, 0x16, 0x16, 0x16))
$glow = New-Object System.Drawing.Drawing2D.GraphicsPath; $glow.AddEllipse(560, -140, 820, 820)
$pgb = New-Object System.Drawing.Drawing2D.PathGradientBrush $glow; $pgb.CenterColor = [System.Drawing.Color]::FromArgb(110, 0x35, 0x7F, 0x87); $pgb.SurroundColors = @([System.Drawing.Color]::FromArgb(0, 0x16, 0x16, 0x16)); $g.FillPath($pgb, $glow)
Fill-Round $g $teal 90 90 120 120 30
$g.DrawString('W', (New-Object System.Drawing.Font 'Segoe UI', 72, ([System.Drawing.FontStyle]::Bold), ([System.Drawing.GraphicsUnit]::Pixel)), [System.Drawing.Brushes]::White, (New-Object System.Drawing.RectangleF 90, 94, 120, 120), $center)
$white = [System.Drawing.Brushes]::White
$muted = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 0xA3, 0xA3, 0xA3))
$accent = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 0x49, 0xA3, 0xAD))
$g.DrawString('Wayne', (New-Object System.Drawing.Font 'Segoe UI', 44, ([System.Drawing.FontStyle]::Bold), ([System.Drawing.GraphicsUnit]::Pixel)), $white, 236, 118)
$g.DrawString('Alt+Espace,', (New-Object System.Drawing.Font 'Segoe UI', 96, ([System.Drawing.FontStyle]::Bold), ([System.Drawing.GraphicsUnit]::Pixel)), $white, 80, 270)
$g.DrawString("et c'est ouvert.", (New-Object System.Drawing.Font 'Segoe UI', 96, ([System.Drawing.FontStyle]::Bold), ([System.Drawing.GraphicsUnit]::Pixel)), $accent, 80, 385)
$g.DrawString("Le lanceur d'applications pour Windows qui apprend tes habitudes · $size", (New-Object System.Drawing.Font 'Segoe UI', 30, ([System.Drawing.FontStyle]::Regular), ([System.Drawing.GraphicsUnit]::Pixel)), $muted, 86, 530)
$b.Save("$PWD\$out\og.png", [System.Drawing.Imaging.ImageFormat]::Png); $g.Dispose(); $b.Dispose()

Write-Host "OK : $out\ (version $version, $size, URL $Url)"
Get-ChildItem $out -Recurse -File | ForEach-Object { "  $($_.FullName.Substring($PWD.Path.Length + 1))  $([math]::Round($_.Length / 1KB)) Ko" }
