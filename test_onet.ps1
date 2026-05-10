# Test onet.pl
Get-Process chrome -EA SilentlyContinue | Stop-Process -Force
Remove-Item "$env:TEMP\rust_browser_mcp_profile" -Recurse -Force -EA SilentlyContinue

$exe = "C:\rust-browser\target\release\rust-browser-mcp.exe"
$psi = New-Object System.Diagnostics.ProcessStartInfo($exe)
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$proc = [System.Diagnostics.Process]::Start($psi)

$q = [System.Collections.Concurrent.ConcurrentQueue[string]]::new()
$ql = [System.Collections.Concurrent.ConcurrentQueue[string]]::new()
$rs = [runspacefactory]::CreateRunspace(); $rs.Open()
$rs.SessionStateProxy.SetVariable('r', $proc.StandardOutput)
$rs.SessionStateProxy.SetVariable('q', $q)
$bg = [powershell]::Create(); $bg.Runspace = $rs
$bg.AddScript({ try { while($true){ $ln=$r.ReadLine(); if($null -eq $ln){break}; $q.Enqueue($ln) } } catch{} }) | Out-Null
$bg.BeginInvoke() | Out-Null

# Stderr (logi) w osobnym runspace
$rs2 = [runspacefactory]::CreateRunspace(); $rs2.Open()
$rs2.SessionStateProxy.SetVariable('e', $proc.StandardError)
$rs2.SessionStateProxy.SetVariable('ql', $ql)
$bg2 = [powershell]::Create(); $bg2.Runspace = $rs2
$bg2.AddScript({ try { while($true){ $ln=$e.ReadLine(); if($null -eq $ln){break}; $ql.Enqueue($ln) } } catch{} }) | Out-Null
$bg2.BeginInvoke() | Out-Null

function Send-Msg($obj) { $proc.StandardInput.WriteLine(($obj | ConvertTo-Json -Compress)) }
function Read-Resp([int]$ms=30000) {
    $deadline = [DateTime]::Now.AddMilliseconds($ms)
    $ln = $null
    while([DateTime]::Now -lt $deadline) {
        if($q.TryDequeue([ref]$ln)) { return $ln }
        Start-Sleep -Milliseconds 50
    }
    return $null
}

Send-Msg @{jsonrpc="2.0";id=1;method="initialize";params=@{protocolVersion="2024-11-05";capabilities=@{};clientInfo=@{name="test";version="1.0"}}}
$r1 = Read-Resp 5000
Write-Host "INIT: OK"
Send-Msg @{jsonrpc="2.0";method="notifications/initialized";params=@{}}

Write-Host "Navigating https://www.onet.pl/ (max 60s)..."
Send-Msg @{jsonrpc="2.0";id=2;method="tools/call";params=@{name="navigate";arguments=@{url="https://www.onet.pl/"}}}
$r2 = Read-Resp 60000

if ($r2) {
    $obj = $r2 | ConvertFrom-Json
    $text = $obj.result.content[0].text
    $len = $text.Length
    Write-Host ""
    Write-Host "=== WAP-XML z onet.pl ($len znakow) ==="
    Write-Host $text.Substring(0, [Math]::Min(4000, $len))
    if ($len -gt 4000) { Write-Host "...[skrocono, calkowita dlugosc: $len znakow]..." }
} else {
    Write-Host "TIMEOUT lub blad"
}

$proc.Kill()
