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

# the job must run even when laplace is unreachable
function Send-Ping([string] $Result) {
    try {
        Invoke-RestMethod -Method Post -Uri "$Laplace/ping/$Flow/${Result}?$query" -Headers $headers -TimeoutSec 10 | Out-Null
    } catch {
        Write-Warning "laplace unreachable: $_"
    }
}

Send-Ping 'start'
$process = Start-Process -FilePath $Command -ArgumentList $Arguments -NoNewWindow -Wait -PassThru
Send-Ping $process.ExitCode
exit $process.ExitCode
