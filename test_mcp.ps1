# Test MCP server - symuluje Claude Desktop (JSON-RPC 2.0 over stdio)

$exe = "C:\rust-browser\target\release\rust-browser-mcp.exe"

if (-not (Test-Path $exe)) {
    Write-Host "BLAD: brak binarki $exe" -ForegroundColor Red; exit 1
}

Write-Host "Startuje: $exe" -ForegroundColor Cyan

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName               = $exe
$psi.UseShellExecute        = $false
$psi.RedirectStandardInput  = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError  = $true
$psi.CreateNoWindow         = $true

$proc = New-Object System.Diagnostics.Process
$proc.StartInfo = $psi
$proc.Start() | Out-Null

# Kolejka odpowiedzi - czyta stdout w osobnym Runspace, unika "stream in use"
$responseQueue = [System.Collections.Concurrent.ConcurrentQueue[string]]::new()
$rs = [runspacefactory]::CreateRunspace()
$rs.Open()
$rs.SessionStateProxy.SetVariable('rdr', $proc.StandardOutput)
$rs.SessionStateProxy.SetVariable('q',   $responseQueue)
$bgPs = [powershell]::Create(); $bgPs.Runspace = $rs
$bgPs.AddScript({
    try { while ($true) { $ln = $rdr.ReadLine(); if ($null -eq $ln) { break }; $q.Enqueue($ln) } } catch {}
}) | Out-Null
$bgAsync = $bgPs.BeginInvoke()

# Stderr (tracing logi) asynchronicznie
$proc.BeginErrorReadLine() 2>$null
$null = Register-ObjectEvent $proc ErrorDataReceived -Action {
    if ($Event.SourceEventArgs.Data) {
        Write-Host "[LOG] $($Event.SourceEventArgs.Data)" -ForegroundColor DarkGray
    }
}

function Send-Rpc($obj) {
    $json = $obj | ConvertTo-Json -Depth 10 -Compress
    Write-Host "> $json" -ForegroundColor Yellow
    $proc.StandardInput.WriteLine($json)
    $proc.StandardInput.Flush()
}

function Read-Rpc([int]$ms = 30000) {
    $deadline = [DateTime]::UtcNow.AddMilliseconds($ms)
    while ([DateTime]::UtcNow -lt $deadline) {
        $line = $null
        if ($responseQueue.TryDequeue([ref]$line)) {
            if ($line) {
                Write-Host "< $line" -ForegroundColor Green
                return $line | ConvertFrom-Json -ErrorAction SilentlyContinue
            }
        }
        Start-Sleep -Milliseconds 50
    }
    Write-Host "TIMEOUT (${ms}ms)" -ForegroundColor Red
    return $null
}

# ================================================================
Write-Host "`n=== 1. Initialize ===" -ForegroundColor Magenta
Send-Rpc @{
    jsonrpc = "2.0"; id = 1; method = "initialize"
    params = @{
        protocolVersion = "2024-11-05"
        capabilities    = @{}
        clientInfo      = @{ name = "test"; version = "1.0" }
    }
}
$r = Read-Rpc 5000
if (-not $r) { Write-Host "Brak odpowiedzi - serwer nie startuje?"; $proc.Kill(); exit 1 }
Write-Host "OK: $($r.result.serverInfo.name) v$($r.result.serverInfo.version)" -ForegroundColor Green

Send-Rpc @{ jsonrpc = "2.0"; method = "notifications/initialized"; params = @{} }
Start-Sleep -Milliseconds 500

# ================================================================
Write-Host "`n=== 2. tools/list ===" -ForegroundColor Magenta
Send-Rpc @{ jsonrpc = "2.0"; id = 2; method = "tools/list"; params = @{} }
$r = Read-Rpc 10000
if ($r) {
    $narzedzia = $r.result.tools | ForEach-Object { $_.name }
    Write-Host "Tools ($($narzedzia.Count)): $($narzedzia -join ', ')" -ForegroundColor Green
}

# ================================================================
Write-Host "`n=== 3. navigate -> example.com ===" -ForegroundColor Magenta
Write-Host "(Uruchamia Chrome, moze zajac 15-30s na pierwszym starcie...)" -ForegroundColor Gray
Send-Rpc @{
    jsonrpc = "2.0"; id = 3; method = "tools/call"
    params  = @{ name = "navigate"; arguments = @{ url = "https://example.com/" } }
}
$r = Read-Rpc 60000
if ($r -and $r.result) {
    $xml = $r.result.content[0].text
    Write-Host "Otrzymano $($xml.Length) znakow WAP-XML:" -ForegroundColor Green
    Write-Host ($xml.Substring(0, [Math]::Min(600, $xml.Length))) -ForegroundColor White
} elseif ($r -and $r.error) {
    Write-Host "BLAD: $($r.error.message)" -ForegroundColor Red
}

# ================================================================
Write-Host "`n=== 4. get_text (czysty tekst) ===" -ForegroundColor Magenta
Send-Rpc @{
    jsonrpc = "2.0"; id = 4; method = "tools/call"
    params  = @{ name = "get_text"; arguments = @{} }
}
$r = Read-Rpc 15000
if ($r -and $r.result) {
    Write-Host $r.result.content[0].text -ForegroundColor White
}

# ================================================================
Write-Host "`n=== 5. screenshot -> example.com ===" -ForegroundColor Magenta
Send-Rpc @{
    jsonrpc = "2.0"; id = 5; method = "tools/call"
    params  = @{ name = "screenshot"; arguments = @{ url = "https://example.com/" } }
}
$r = Read-Rpc 30000
if ($r -and $r.result) {
    $text = $r.result.content[0].text
    try {
        $data = $text | ConvertFrom-Json
        # VerifyResult: {url, screenshot_base64, mime}
        $b64 = if ($data.screenshot_base64) { $data.screenshot_base64 } elseif ($data.base64) { $data.base64 } else { $null }
        if ($b64) {
            Write-Host "Screenshot OK: $($b64.Length) chars base64 PNG" -ForegroundColor Green
            $bytes = [Convert]::FromBase64String($b64)
            [IO.File]::WriteAllBytes("C:\rust-browser\test_screenshot.png", $bytes)
            Write-Host "Zapisano: C:\rust-browser\test_screenshot.png" -ForegroundColor Cyan
        } else {
            Write-Host "Screenshot: brak base64 w odpowiedzi" -ForegroundColor Yellow
            Write-Host $text
        }
    } catch {
        Write-Host "Screenshot raw: $($text.Substring(0,[Math]::Min(200,$text.Length)))" -ForegroundColor Yellow
    }
}

# ================================================================
Write-Host "`n=== 6. Stealth test -> bot.sannysoft.com ===" -ForegroundColor Magenta
Write-Host "(PASSED=niewidoczny bot, FAILED=wykryty)" -ForegroundColor Gray
Send-Rpc @{
    jsonrpc = "2.0"; id = 6; method = "tools/call"
    params  = @{ name = "navigate"; arguments = @{ url = "https://bot.sannysoft.com/" } }
}
$r = Read-Rpc 60000
if ($r -and $r.result) {
    $xml = $r.result.content[0].text
    $failed = ([regex]::Matches($xml, "FAILED")).Count
    $passed = ([regex]::Matches($xml, "PASSED")).Count
    $col = if ($failed -eq 0) { "Green" } else { "Yellow" }
    Write-Host "Wynik stealth: PASSED=$passed  FAILED=$failed" -ForegroundColor $col
    Write-Host ($xml.Substring(0, [Math]::Min(1000, $xml.Length))) -ForegroundColor White
}

# ================================================================
Write-Host "`n=== Koniec testow ===" -ForegroundColor Magenta
$proc.Kill()
Write-Host "Sprawdz plik test_screenshot.png" -ForegroundColor Cyan
