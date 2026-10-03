# wraps a task scheduler job so laplace knows when it started, finished and how it exited,
# and on a failure the last 100 lines it wrote to stderr (or stdout, when stderr is empty).
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
    [string] $Token = $env:LAPLACE_TOKEN,
    [switch] $KeepOutput
)

$query = "every=$Every"
if ($Cron) { $query = "cron=$([uri]::EscapeDataString($Cron))" }
if ($Calendar) { $query += "&calendar=$Calendar" }

$headers = @{}
if ($Token) { $headers.Authorization = "Bearer $Token" }

# the job must run even when laplace is unreachable; a report is tried three times, since a
# database failover behind laplace can fail one for a few seconds
function Send-Ping([string] $Result, [string] $ErrorText) {
    $request = @{ Method = 'Post'; Uri = "$Laplace/ping/$Flow/${Result}?$query"; Headers = $headers; TimeoutSec = 10 }
    if ($ErrorText) {
        $request.Body = [Text.Encoding]::UTF8.GetBytes((@{ error = $ErrorText } | ConvertTo-Json -Compress))
        $request.ContentType = 'application/json; charset=utf-8'
    }
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        try {
            Invoke-RestMethod @request | Out-Null
            return
        } catch {
            if ($attempt -eq 3) { Write-Warning "laplace unreachable: $_" } else { Start-Sleep -Seconds 2 }
        }
    }
}

# the end of the output is sent with a failure; laplace masks secrets in it, but a job that
# prints data it must not share should run with -KeepOutput
function Get-Tail([string] $Path) {
    if (-not (Test-Path $Path)) { return '' }
    $text = (Get-Content $Path -Tail 100) -join "`n"
    if ($text.Length -gt 16000) { $text = $text.Substring($text.Length - 16000) }
    return $text
}

Send-Ping 'start'
$stdout = [IO.Path]::GetTempFileName()
$stderr = [IO.Path]::GetTempFileName()
# windows powershell refuses an empty -ArgumentList, so it is only passed when there are arguments
$start = @{
    FilePath = $Command; NoNewWindow = $true; Wait = $true; PassThru = $true; ErrorAction = 'Stop'
    RedirectStandardOutput = $stdout; RedirectStandardError = $stderr
}
if ($Arguments.Count -gt 0) { $start.ArgumentList = $Arguments }
try {
    $code = (Start-Process @start).ExitCode
    $errorText = Get-Tail $stderr
    if (-not $errorText) { $errorText = Get-Tail $stdout }
} catch {
    # a job that cannot start has failed, and task scheduler and laplace must both see that
    $errorText = "could not start ${Command}: $_"
    Write-Error $errorText -ErrorAction Continue
    $code = 1
}
# the job's output still reaches whatever logs this task
Get-Content $stdout -ErrorAction SilentlyContinue
Get-Content $stderr -ErrorAction SilentlyContinue | ForEach-Object { [Console]::Error.WriteLine($_) }
Remove-Item $stdout, $stderr -ErrorAction SilentlyContinue

if ($code -ne 0 -and -not $KeepOutput) { Send-Ping $code $errorText } else { Send-Ping $code }
exit $code
