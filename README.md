# rust-browser-mcp

Rust MCP server sterujący Chrome przez CDP. Daje agentom AI narzędzia do przeglądania stron — zwraca semantyczny WAP-XML zamiast surowego HTML.

## Wymagania

- Windows 10/11
- Chrome 115+ (testoowany na 145.0.7632.109)
- Rust (edition 2021)

## Budowanie

```powershell
cd C:\rust-browser
cargo build --release
# binarny: target\release\rust-browser-mcp.exe
```

## Podpięcie pod Claude Desktop

Plik: `%APPDATA%\Claude\claude_desktop_config.json`

```json
{
  "mcpServers": {
    "browser": {
      "command": "C:\\rust-browser\\target\\release\\rust-browser-mcp.exe"
    }
  }
}
```

Chrome uruchamia się leniwie — dopiero przy pierwszym wywołaniu narzędzia. `initialize` i `tools/list` odpowiadają natychmiast.

## Narzędzia (9)

| Narzędzie | Opis |
|-----------|------|
| `navigate` | Otwiera URL, zwraca WAP-XML (tytuł + treść + formularze + linki z ID) |
| `get_text` | Czysty tekst aktualnej strony (nagłówki + akapity, bez tagów) |
| `get_links` | Tylko lista linków aktualnej strony (id, href, label) |
| `get_content` | Pełny WAP-XML aktualnej strony bez nowego requesta |
| `click_link` | Nawiguje do linku po jego ID z WAP-XML |
| `fill_and_submit` | Wypełnia formularz i wysyła, zwraca WAP-XML strony wynikowej |
| `screenshot` | Screenshot PNG jako base64 |
| `get_state` | Aktualny URL i tytuł |
| `solve_captcha` | Rozwiązuje captchę przez 2captcha / anti-captcha / CapMonster |

### Przykład WAP-XML

```xml
<page url="https://www.onet.pl/" title="Onet – Jesteś na bieżąco">
  <h3>Tytuł artykułu...</h3>
  <form id="1" method="GET" action="https://szukaj.onet.pl/wyniki.html">
    <input id="2" name="qt" type="search" placeholder=" " />
    <button id="3">SZUKAJ</button>
  </form>
  <link id="4" href="https://www.onet.pl/">/</link>
  <link id="10" href="https://wiadomosci.onet.pl/">WIADOMOŚCI</link>
</page>
```

## Architektura

```
MCP (JSON-RPC 2.0 / stdio)
        │
   McpServer
   (lazy init)
        │
   AgentBrowser
        │
   Chrome CDP (chromiumoxide 0.7)
        │
   Chrome headless (manual spawn)
```

### Uruchamianie Chrome

`Browser::launch()` z chromiumoxide nie działa na Chrome 145 / Windows (exit code 21). Zamiast tego:

1. `TcpListener::bind("127.0.0.1:0")` → wolny port → drop
2. `std::process::Command::spawn()` z `--remote-debugging-port=PORT`
3. `.stdin(Stdio::null())` — **krytyczne**: bez tego Chrome dziedziczy stdin MCP servera i od razu kończy działanie
4. Poll `GET /json/version HTTP/1.1` przez raw `tokio::net::TcpStream` z `timeout(500ms)` na `read_to_end` — Chrome HTTP/1.1 nie zamyka połączenia mimo `Connection: close`, więc `read_to_end` bez timeoutu wisi w nieskończoność
5. `Browser::connect(ws_url)` przez chromiumoxide

### Stealth

Chrome uruchamiany z:
- `--disable-blink-features=AutomationControlled`
- User-Agent bez "HeadlessChrome"
- `--lang=pl-PL,pl`

Plus STEALTH_JS wstrzykiwany na każdej stronie (nadpisuje `navigator.webdriver`, `navigator.plugins`, `window.chrome`, CDP artifacts).

### Lazy init

Chrome startuje dopiero przy pierwszym wywołaniu narzędzia:

```rust
pub struct McpServer {
    browser: Arc<Mutex<Option<AgentBrowser>>>,
}
```

`initialize` i `tools/list` odpowiadają natychmiast, bez uruchamiania Chrome.

## Testowanie

```powershell
# Zabij stare procesy Chrome, wyczyść profil
Get-Process chrome -EA SilentlyContinue | Stop-Process -Force
Remove-Item "$env:TEMP\rust_browser_mcp_profile" -Recurse -Force -EA SilentlyContinue

# Uruchom testy
powershell -ExecutionPolicy Bypass -File .\test_mcp.ps1
```

Oczekiwany wynik:
```
=== 1. Initialize ===   OK: rust-browser-mcp v0.1.0
=== 2. tools/list ===   Tools (9): navigate, get_text, ...
=== 3. navigate ===     <page url="https://example.com/" ...>
=== 4. get_text ===     # Example Domain ...
=== 5. screenshot ===   Screenshot OK: ~14000 chars base64 PNG
=== 6. Stealth test === PASSED=0 FAILED=0
```

## Rozwiązane problemy

| Problem | Przyczyna | Rozwiązanie |
|---------|-----------|-------------|
| Chrome exit code 21 | `--remote-debugging-pipe` incompatible z Chrome 145 | Manual spawn + `Browser::connect(ws_url)` |
| Chrome nie drukuje WS URL | Flagi `--log-level=3` tłumiły logi | Usunięcie flag + HTTP polling |
| `read_to_end` wiesza się | Chrome HTTP/1.1 nie zamyka połączenia | `timeout(500ms)` na `read_to_end` |
| Chrome kończy natychmiast | Dziedziczy stdin MCP servera (pipe) | `.stdin(Stdio::null())` |
| "Multiple targets not supported" | Zombie Chrome trzyma profil | `remove_dir_all(profile)` przed startem |
| Zombie Chrome po timeout | `Child::drop()` na Windows nie killuje | Explicit `child.kill()` |
| Deadlock w dispatch_tool | Dwa `lock().await` na tym samym Mutex | Jeden guard obejmuje init i użycie |
