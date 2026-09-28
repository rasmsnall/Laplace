# wraps a task scheduler job so laplace knows when it started, finished and how it exited.
# the first run creates the flow; -Every says how often laplace should expect it (default 1d),
# or -Cron gives the task's own schedule, with -Calendar se or fi to skip that country's holidays.
# action: powershell.exe -NoProfile -File heartbeat.ps1 -Flow nightly-reconcile -Every 1d -Command C:\jobs\reconcile.exe
param(
    [Parameter(Mandatory)] [string] $Flow,
    [Parameter(Mandatory)] [string] $Command,
    [string[]] $Arguments = @(),
    [string] $Every = '1d',
    [string] $Cron,
    [string] $Calendar,
    [string] $Laplace = $env:LAPLACE_URL,
    [string] $Token = $env:LAPLACE_TOKEN
)

$query = "every=$Every"
if ($Cron) { $query = "cron=$([uri]::EscapeDataString($Cron))" }
if ($Calendar) { $query += "&calendar=$Calendar" }

$headers = @{}
if ($Token) { $headers.Authorization = "Bearer $Token" }

# the job must run even when laplace is unreachable; a report is tried three times, since a
# database failover behind laplace can fail one for a few seconds
function Send-Ping([string] $Result) {
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        try {
            Invoke-RestMethod -Method Post -Uri "$Laplace/ping/$Flow/${Result}?$query" -Headers $headers -TimeoutSec 10 | Out-Null
            return
        } catch {
            if ($attempt -eq 3) { Write-Warning "laplace unreachable: $_" } else { Start-Sleep -Seconds 2 }
        }
    }
}

Send-Ping 'start'
# windows powershell refuses an empty -ArgumentList, so it is only passed when there are arguments
$start = @{ FilePath = $Command; NoNewWindow = $true; Wait = $true; PassThru = $true; ErrorAction = 'Stop' }
if ($Arguments.Count -gt 0) { $start.ArgumentList = $Arguments }
try {
    $code = (Start-Process @start).ExitCode
} catch {
    # a job that cannot start has failed, and task scheduler and laplace must both see that
    Write-Error "could not start ${Command}: $_" -ErrorAction Continue
    $code = 1
}
Send-Ping $code
exit $code
