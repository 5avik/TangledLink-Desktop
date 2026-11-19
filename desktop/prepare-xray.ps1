$ErrorActionPreference = "Stop"
$version = "v26.11.9"
$digest = "1262522266337b824f07beb8e1375786b437683b3890105469088f8e17e0a69d"
$temp = Join-Path ([System.IO.Path]::GetTempPath()) ("tangledlink-xray-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $temp | Out-Null
try {
    $archive = Join-Path $temp "xray.zip"
    Invoke-WebRequest -UseBasicParsing "https://github.com/XTLS/Xray-core/releases/download/$version/Xray-windows-64.zip" -OutFile $archive
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $digest) {
        throw "Xray archive checksum mismatch"
    }
    $files = Join-Path $temp "files"
    Expand-Archive -LiteralPath $archive -DestinationPath $files
    $target = Join-Path $PSScriptRoot "xray"
    New-Item -ItemType Directory -Path $target -Force | Out-Null
    foreach ($file in "xray.exe", "wintun.dll", "LICENSE", "LICENSE-Wintun") {
        Copy-Item -LiteralPath (Join-Path $files $file) -Destination $target -Force
    }
} finally {
    Remove-Item -LiteralPath $temp -Recurse -Force
}
