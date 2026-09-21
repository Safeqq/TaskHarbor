[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$DatabaseUrl,

    [Parameter(Mandatory = $true)]
    [string]$StorageDir,

    [Parameter(Mandatory = $true)]
    [string]$Destination,

    [Parameter(Mandatory = $true)]
    [switch]$ConfirmQuiesced,

    [string]$PgBin = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-PostgresTool {
    param([string]$Name, [string]$PgBin)

    if ($PgBin) {
        $candidate = Join-Path ([System.IO.Path]::GetFullPath($PgBin)) $Name
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
        throw "$Name was not found in PgBin."
    }
    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }
    throw "$Name was not found. Pass the PostgreSQL bin directory through -PgBin."
}

if (-not $ConfirmQuiesced) {
    throw "Stop the API and every worker, then pass -ConfirmQuiesced."
}

$storagePath = (Resolve-Path -LiteralPath $StorageDir).Path
$destinationPath = [System.IO.Path]::GetFullPath($Destination)
if ($destinationPath.StartsWith($storagePath + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Destination must be outside the storage directory."
}
if (Test-Path -LiteralPath $destinationPath) {
    if (@(Get-ChildItem -LiteralPath $destinationPath -Force).Count -ne 0) {
        throw "Destination must be absent or empty."
    }
} else {
    New-Item -ItemType Directory -Path $destinationPath | Out-Null
}

$pgDump = Resolve-PostgresTool -Name "pg_dump.exe" -PgBin $PgBin
$databaseArchive = Join-Path $destinationPath "database.dump"
$storageArchive = Join-Path $destinationPath "storage.zip"

& $pgDump "--dbname=$DatabaseUrl" "--format=custom" "--file=$databaseArchive" "--no-owner" "--no-privileges"
if ($LASTEXITCODE -ne 0) {
    throw "pg_dump failed with exit code $LASTEXITCODE."
}

Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory(
    $storagePath,
    $storageArchive,
    [System.IO.Compression.CompressionLevel]::Optimal,
    $false
)

$manifest = [ordered]@{
    format_version = 1
    created_at_utc = [DateTime]::UtcNow.ToString("o")
    quiesced = $true
    database_archive = [ordered]@{
        name = "database.dump"
        sha256 = (Get-FileHash -LiteralPath $databaseArchive -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    storage_archive = [ordered]@{
        name = "storage.zip"
        sha256 = (Get-FileHash -LiteralPath $storageArchive -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}
$manifest | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $destinationPath "manifest.json") -Encoding UTF8

Write-Output "Backup created at $destinationPath"
