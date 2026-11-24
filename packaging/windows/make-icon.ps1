# SPDX-License-Identifier: GPL-3.0-or-later
# Builds src-tauri/icons/icon.ico, the icon Windows shows for opensidian.exe, its
# window, its taskbar button and its installer, from the PNGs render-icons.sh
# already renders from the one official logo (packaging/linux/opensidian.svg):
#   48, 64, 128, 256 px   copied byte for byte from packaging/linux/icons/
#   16, 24, 32 px         scaled down from 256.png (no such PNG is committed)
# An .ico is a small directory followed by the images; every entry here is a
# PNG, which Windows Vista and later read at any size. Nothing but Windows'
# own System.Drawing is used. Swapping the logo = re-run render-icons.sh, then
# this, and commit the output.
#
#   powershell -ExecutionPolicy Bypass -File packaging/windows/make-icon.ps1
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$src = Join-Path $root 'packaging/linux/icons'
$out = Join-Path $root 'src-tauri/icons/icon.ico'

function Scaled([string]$png, [int]$size) {
    $img = [System.Drawing.Image]::FromFile($png)
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $g.DrawImage($img, 0, 0, $size, $size)
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose(); $img.Dispose()
    return ,$ms.ToArray()
}

$entries = @()
foreach ($s in 16, 24, 32) { $entries += ,@($s, (Scaled (Join-Path $src '256.png') $s)) }
foreach ($s in 48, 64, 128, 256) { $entries += ,@($s, [System.IO.File]::ReadAllBytes((Join-Path $src "$s.png"))) }

$ms = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $ms
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$entries.Count)   # reserved, type 1 = icon, count
$offset = 6 + 16 * $entries.Count
foreach ($e in $entries) {
    $size = $e[0]; $bytes = $e[1]
    $dim = if ($size -ge 256) { 0 } else { $size }                             # 0 means 256
    $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)                                 # planes, bits per pixel
    $w.Write([uint32]$bytes.Length); $w.Write([uint32]$offset)
    $offset += $bytes.Length
}
foreach ($e in $entries) { $w.Write([byte[]]$e[1]) }
$w.Flush()
[System.IO.File]::WriteAllBytes($out, $ms.ToArray())
"wrote $out ({0} bytes, sizes: {1})" -f $ms.Length, (($entries | ForEach-Object { $_[0] }) -join ', ')
