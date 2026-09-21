[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$BackupDir,

    [Parameter(Mandatory = $true)]
    [string]$DatabaseUrl,

    [Parameter(Mandatory = $true)]
    [string]$StorageDir,

    [Parameter(Mandatory = $true)]
    [switch]$ConfirmQuiesced,

    [string]$PgBin = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Assert-Checksum {
    param([string]$Path, [string]$Expected)

    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $Expected.ToLowerInvariant()) {
        throw "Checksum verification failed for $(Split-Path -Leaf $Path)."
    }
}

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
    throw "Keep the API and every worker stopped, then pass -ConfirmQuiesced."
}

$backupPath = (Resolve-Path -LiteralPath $BackupDir).Path
$manifestPath = Join-Path $backupPath "manifest.json"
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($manifest.format_version -ne 1 -or -not $manifest.quiesced) {
    throw "The backup manifest is unsupported or was not marked quiesced."
}
$databaseArchive = Join-Path $backupPath $manifest.database_archive.name
$storageArchive = Join-Path $backupPath $manifest.storage_archive.name
Assert-Checksum -Path $databaseArchive -Expected $manifest.database_archive.sha256
Assert-Checksum -Path $storageArchive -Expected $manifest.storage_archive.sha256

$storagePath = [System.IO.Path]::GetFullPath($StorageDir)
if (Test-Path -LiteralPath $storagePath) {
    if (@(Get-ChildItem -LiteralPath $storagePath -Force).Count -ne 0) {
        throw "StorageDir must be absent or empty."
    }
} else {
    New-Item -ItemType Directory -Path $storagePath | Out-Null
}

Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::OpenRead($storageArchive)
try {
    $storagePrefix = $storagePath.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
    foreach ($entry in $archive.Entries) {
        $entryPath = [System.IO.Path]::GetFullPath((Join-Path $storagePath $entry.FullName))
        if (-not $entryPath.StartsWith($storagePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "The storage archive contains an unsafe path."
        }
    }
} finally {
    $archive.Dispose()
}
[System.IO.Compression.ZipFile]::ExtractToDirectory($storageArchive, $storagePath)

$pgRestore = Resolve-PostgresTool -Name "pg_restore.exe" -PgBin $PgBin
& $pgRestore "--dbname=$DatabaseUrl" "--clean" "--if-exists" "--no-owner" "--no-privileges" $databaseArchive
if ($LASTEXITCODE -ne 0) {
    throw "pg_restore failed with exit code $LASTEXITCODE."
}

Write-Output "Backup restored into the requested database and storage directory."
