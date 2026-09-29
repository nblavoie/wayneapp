# Génère assets\wayne.ico : carré arrondi turquoise avec un « W » blanc (PNG 16 → 256 px).
Add-Type -AssemblyName System.Drawing
$sizes = 16, 20, 24, 32, 40, 48, 64, 256
$pngs = foreach ($s in $sizes) {
    $bmp = New-Object System.Drawing.Bitmap $s, $s
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'; $g.TextRenderingHint = 'AntiAliasGridFit'; $g.PixelOffsetMode = 'Half'
    $inset = [math]::Max(0.5, $s * 0.03); $w = $s - 2 * $inset; $r = $w * 0.26; $d = 2 * $r
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $p.AddArc($inset, $inset, $d, $d, 180, 90); $p.AddArc($inset + $w - $d, $inset, $d, $d, 270, 90)
    $p.AddArc($inset + $w - $d, $inset + $w - $d, $d, $d, 0, 90); $p.AddArc($inset, $inset + $w - $d, $d, $d, 90, 90); $p.CloseFigure()
    $g.FillPath((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 0x35, 0x7F, 0x87))), $p)
    $font = New-Object System.Drawing.Font 'Segoe UI', ($s * 0.56), ([System.Drawing.FontStyle]::Bold), ([System.Drawing.GraphicsUnit]::Pixel)
    $fmt = New-Object System.Drawing.StringFormat; $fmt.Alignment = 'Center'; $fmt.LineAlignment = 'Center'
    $g.DrawString('W', $font, [System.Drawing.Brushes]::White, (New-Object System.Drawing.RectangleF 0, ($s * 0.02), $s, $s), $fmt)
    $g.Dispose()
    $ms = New-Object IO.MemoryStream; $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
    , $ms.ToArray()
}
$out = New-Object IO.MemoryStream; $bw = New-Object IO.BinaryWriter $out
$bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $s = $sizes[$i]; $dim = if ($s -ge 256) { 0 } else { $s }
    $bw.Write([byte]$dim); $bw.Write([byte]$dim); $bw.Write([byte]0); $bw.Write([byte]0)
    $bw.Write([uint16]1); $bw.Write([uint16]32); $bw.Write([uint32]$pngs[$i].Length); $bw.Write([uint32]$offset)
    $offset += $pngs[$i].Length
}
foreach ($png in $pngs) { $bw.Write($png) }
$bw.Flush(); [IO.File]::WriteAllBytes((Join-Path $PSScriptRoot 'wayne.ico'), $out.ToArray())
