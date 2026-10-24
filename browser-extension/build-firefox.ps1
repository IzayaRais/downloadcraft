# Builds the Firefox add-on (.xpi) from this folder.
#
# Firefox wants the manifest to be called manifest.json, while the Chrome
# build keeps it at manifest.json too - so both manifests live here under
# distinct names (manifest.json / manifest-firefox.json) and this script
# stages the shared sources with the Firefox one renamed. The add-on ships
# the same readable files as the Chrome build: no bundler, no minifier.
#
#   powershell -ExecutionPolicy Bypass -File build-firefox.ps1
#
# The result is DownloadCraft-Firefox-<version>.xpi next to the repository
# root, ready for about:debugging ("Load Temporary Add-on").

param([string]$Out = "")

$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$ver = (Get-Content (Join-Path $here "manifest-firefox.json") -Raw | ConvertFrom-Json).version
if (-not $Out) {
  $Out = Join-Path (Split-Path -Parent $here) "DownloadCraft-Firefox-$ver.xpi"
}

$stage = Join-Path $env:TEMP "download-craft-firefox-stage"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path (Join-Path $stage "icons") -Force | Out-Null

foreach ($f in "rank.js", "streams.js", "mp4merge.js", "browserdl.js", "background.js", "content.js", "page-hook.js", "page-relay.js", "popup.html", "popup.js") {
  Copy-Item (Join-Path $here $f) $stage
}
Copy-Item (Join-Path $here "icons\*.png") (Join-Path $stage "icons")
Copy-Item (Join-Path $here "manifest-firefox.json") (Join-Path $stage "manifest.json")

if (Test-Path $Out) { Remove-Item $Out -Force }
# Compress-Archive only writes .zip, so build that and give it the .xpi name
# Firefox expects (an .xpi is a plain zip).
if ($Out.ToLower().EndsWith(".xpi")) {
  $zip = [System.IO.Path]::ChangeExtension($Out, ".zip")
  if (Test-Path $zip) { Remove-Item $zip -Force }
  Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip -CompressionLevel Optimal
  Move-Item $zip $Out -Force
} else {
  Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $Out -CompressionLevel Optimal
}

Write-Host ("Built {0} ({1} bytes)" -f $Out, (Get-Item $Out).Length)
Write-Host ("Staged sources kept at {0} for web-ext lint" -f $stage)
