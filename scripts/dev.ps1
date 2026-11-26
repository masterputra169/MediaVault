$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$backend = Join-Path $root 'backend'
$frontend = Join-Path $root 'frontend'
$python = Join-Path $backend '.venv/Scripts/python.exe'

if (!(Test-Path -LiteralPath $python) -or !(Test-Path -LiteralPath (Join-Path $frontend 'node_modules/next'))) {
    throw 'Pasang dependency backend dan frontend sesuai README.md.'
}

$env:NEXT_TELEMETRY_DISABLED = '1'
$server = Start-Process -FilePath $python -ArgumentList '-m', 'app.serve' -WorkingDirectory $backend -WindowStyle Hidden -PassThru
Push-Location -LiteralPath $frontend
try {
    Start-Sleep -Milliseconds 1200
    if ($server.HasExited) { throw 'Backend gagal dimulai. Jalankan python -m app.serve di backend untuk diagnosis.' }
    pnpm dev
    if ($LASTEXITCODE -ne 0) { throw "Frontend berhenti dengan exit code $LASTEXITCODE." }
}
finally {
    Pop-Location
    if (!$server.HasExited) { Stop-Process -Id $server.Id }
    $server.Dispose()
}
