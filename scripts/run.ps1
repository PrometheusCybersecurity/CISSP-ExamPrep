# scripts\run.ps1
#
# One-command launcher for CISSP Coach.
#   .\scripts\run.ps1            # cargo run (debug)
#   .\scripts\run.ps1 -Release   # build release binary, run it
#
# After a short delay, opens http://127.0.0.1:7878 in your default browser.

[CmdletBinding()]
param(
  [switch]$Release,
  [string]$Url = 'http://127.0.0.1:7878'
)

$ErrorActionPreference = 'Stop'
$repo = Resolve-Path (Join-Path $PSScriptRoot '..')

if (-not (Test-Path (Join-Path $repo '.env'))) {
  Write-Host "⚠️  No .env found. Copying .env.example -> .env (edit it to add your API key)."
  Copy-Item (Join-Path $repo '.env.example') (Join-Path $repo '.env')
}

# Open the browser shortly after the server starts.
Start-Job -ScriptBlock {
  param($u)
  Start-Sleep -Seconds 2
  Start-Process $u
} -ArgumentList $Url | Out-Null

Push-Location $repo
try {
  if ($Release) {
    cargo run --release
  } else {
    cargo run
  }
} finally {
  Pop-Location
}
