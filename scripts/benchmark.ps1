[CmdletBinding()]
param(
    [string]$ApiUrl = "http://127.0.0.1:3000",
    [int]$Jobs = 12,
    [int]$SubmitConcurrency = 4,
    [int]$WorkerConcurrency = 2,
    [string]$Output = "var/benchmarks/latest.json",
    [int]$ApiProcessId = 0,
    [int[]]$WorkerProcessId = @()
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not $env:TASKHARBOR_OWNER_PASSWORD) {
    throw "Set TASKHARBOR_OWNER_PASSWORD before running the benchmark."
}
if ($Jobs -le 0 -or $SubmitConcurrency -le 0 -or $WorkerConcurrency -le 0) {
    throw "Jobs and concurrency values must be greater than zero."
}

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$resultPath = if ([System.IO.Path]::IsPathRooted($Output)) {
    [System.IO.Path]::GetFullPath($Output)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $Output))
}
$env:BENCHMARK_API_URL = $ApiUrl
$env:BENCHMARK_JOBS = $Jobs.ToString()
$env:BENCHMARK_SUBMIT_CONCURRENCY = $SubmitConcurrency.ToString()
$env:TASKHARBOR_WORKER_CONCURRENCY = $WorkerConcurrency.ToString()
$env:BENCHMARK_OUTPUT = $resultPath
if ($ApiUrl.StartsWith("https://localhost", [System.StringComparison]::OrdinalIgnoreCase) -or
    $ApiUrl.StartsWith("https://127.0.0.1", [System.StringComparison]::OrdinalIgnoreCase)) {
    $env:NODE_TLS_REJECT_UNAUTHORIZED = "0"
}

$startInfo = [System.Diagnostics.ProcessStartInfo]::new()
$startInfo.FileName = "node"
$startInfo.Arguments = "scripts/benchmark.mjs"
$startInfo.WorkingDirectory = $repositoryRoot
$startInfo.UseShellExecute = $false
$startInfo.CreateNoWindow = $true
$client = [System.Diagnostics.Process]::new()
$client.StartInfo = $startInfo
if (-not $client.Start()) {
    throw "Benchmark client could not be started."
}

$apiPeak = 0L
$workerPeak = 0L
do {
    if ($ApiProcessId -gt 0) {
        $apiProcess = Get-Process -Id $ApiProcessId -ErrorAction SilentlyContinue
        if ($apiProcess) {
            $apiPeak = [Math]::Max($apiPeak, $apiProcess.WorkingSet64)
        }
    }
    $workerTotal = 0L
    foreach ($processId in $WorkerProcessId) {
        $workerProcess = Get-Process -Id $processId -ErrorAction SilentlyContinue
        if ($workerProcess) {
            $workerTotal += $workerProcess.WorkingSet64
        }
    }
    $workerPeak = [Math]::Max($workerPeak, $workerTotal)
    Start-Sleep -Milliseconds 100
    $client.Refresh()
} while (-not $client.HasExited)

$client.WaitForExit()
$clientExitCode = $client.ExitCode
if ($clientExitCode -ne 0) {
    throw "Benchmark client failed with exit code $clientExitCode."
}

$report = Get-Content -LiteralPath $resultPath -Raw | ConvertFrom-Json
$report.results.memory = [ordered]@{
    measurement = "PowerShell Get-Process.WorkingSet64 sampled every 100 ms"
    api_peak_working_set_bytes = if ($ApiProcessId -gt 0) { $apiPeak } else { $null }
    workers_peak_combined_working_set_bytes = if ($WorkerProcessId.Count -gt 0) { $workerPeak } else { $null }
}
$report | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $resultPath -Encoding UTF8
Write-Output "Memory measurements added to $resultPath"
