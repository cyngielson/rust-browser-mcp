# 🔍 Analiza rust-browser MCP - Szczegółowa

## 📊 Podsumowanie Wykonawcze

**rust-browser MCP** to solidny serwer MCP do kontroli Chrome przez CDP z unikalnym **stealth mode** i **WAP-XML format**. System ma kilka krytycznych problemów do naprawy ale ogromny potencjał.

---

## ✅ Mocne Strony

### 1. Stealth Mode ⭐⭐⭐⭐⭐
- 7 warstw maskowania headless Chrome
- Polskie locale: `--lang=pl-PL,pl`
- User-Agent bez "HeadlessChrome"
- CDP artifacts cleanup

### 2. WAP-XML Format ⭐⭐⭐⭐⭐
- Reader Mode heurystyka (article, main, role="main")
- Deduplikacja treści (seenText Set)
- Filtracja szumu (nav, footer, reklamy, cookie, popup)
- Numerowane ID dla elementów interaktywnych

### 3. Lazy Init ⭐⭐⭐⭐⭐
- Chrome startuje dopiero przy pierwszym tool call
- `initialize` i `tools/list` odpowiadają natychmiast
- Mutex<Option<AgentBrowser>> pattern

### 4. Captcha Solver ⭐⭐⭐⭐
- 3 serwisy: 2captcha, anti-captcha, CapMonster
- 4 typy: image, recaptcha_v2, recaptcha_v3, hcaptcha
- Polling z timeout

### 5. Architektura 3-LLM ⭐⭐⭐⭐
- LLM1 (Planner) → semantic XML
- LLM2 (Worker) → MCP tools
- LLM3 (Judge) → screenshot base64

---

## ⚠️ Problemy Krytyczne

### 1. Martwy kod: cleaner.rs
- Importuje `scraper` crate którego nie ma w Cargo.toml
- Importuje typy `Element`, `ElementKind`, `FormMethod` których nie ma w types.rs
- Nie jest importowany w main.rs
- **Status:** JS extractor (DOM_EXTRACTOR_JS) robi to samo lepiej

### 2. Brak error handling w STEALTH_JS
```rust
let _ = page.evaluate(STEALTH_JS).await; // Ignorujemy błędy!
```
- Jeśli stealth się nie wstrzyknie, bot detection złapie
- **Fix:** Sprawdzać wynik, retry na failure

### 3. Race condition w lazy init
```rust
let mut browser_guard = self.browser.lock().await;
if browser_guard.is_none() {
    let b = AgentBrowser::new().await?; // Może trwać 15s!
}
```
- **Fix:** Użyć `tokio::sync::OnceCell`

### 4. Brak graceful shutdown
- Chrome process jest `_chrome_process: Option<std::sync::Mutex<std::process::Child>>`
- Drop nie zabija Chrome → zombie processes
- **Fix:** Implement `Drop` dla ChromeBrowser

### 5. Page close po każdej operacji
```rust
let _ = page.close().await; // Zamykamy stronę po navigate!
```
- Nie można kliknąć linku na załadowanej stronie
- Każda operacja otwiera nową stronę
- **Fix:** Trzymać Page w state

### 6. Brak cookie/session persistence
- Fresh profile za każdym razem (`remove_dir_all`)
- Nie można się zalogować i potem nawigować
- **Fix:** Opcja `--persistent-profile`

### 7. Timeout hardcoded
```rust
tokio::time::timeout(Duration::from_secs(15), ...)
tokio::time::sleep(Duration::from_millis(200)).await;
tokio::time::sleep(Duration::from_millis(300)).await;
```
- **Fix:** Konfigurowalne przez env vars

### 8. Brak retry logic
- Chrome launch fail → jeden retry bez sandbox
- Navigate fail → błąd
- **Fix:** Exponential backoff retry

### 9. Tylko Windows support
```rust
let candidates = [
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
];
```
- **Fix:** Dodać Linux i macOS paths

### 10. Brak structured logging
- Tylko stderr
- **Fix:** tracing z file output

---

## 🚀 Propozycje Nowych Funkcjonalności

### HIGH PRIORITY

1. **`scroll` tool** - przewijanie strony (infinite scroll, lazy loading)
2. **`wait_for` tool** - czekanie na element (SPA async loading)
3. **`execute_js` tool** - custom JavaScript (elastyczność)
4. **Multi-tab support** - praca równoległa, porównywanie stron
5. **Cookie management** - session persistence, login flows

### MEDIUM PRIORITY

6. **Network interception** - przechwytywanie requestów, API discovery
7. **Element highlighting** - podświetlanie elementów na screenshot (debug)
8. **PDF generation** - archiwizacja, raporty
9. **Performance metrics** - loadTime, domReady, resources
10. **Cross-platform support** - Linux, macOS auto-detect

### LOW PRIORITY

11. **WebSocket support** - real-time updates
12. **Proxy support** - rotacja IP
13. **Mobile emulation** - responsive testing
14. **Geolocation spoofing** - GPS coordinates
15. **Audio/video capture** - media streams

---

## 📋 Plan Implementacji

### Faza 1: Krytyczne Fixes (1-2 dni)
1. ✅ Usuń martwy `cleaner.rs`
2. ✅ Dodaj error handling do STEALTH_JS
3. ✅ Fix race condition (OnceCell)
4. ✅ Implement Drop (graceful shutdown)
5. ✅ Dodaj retry logic

### Faza 2: Ważne Ulepszenia (2-3 dni)
6. Trzymaj Page w state
7. Cookie/session persistence
8. Konfigurowalne timeouts
9. Cross-platform Chrome detection
10. Structured logging

### Faza 3: Nowe Features (3-5 dni)
11. `scroll` tool
12. `wait_for` tool
13. `execute_js` tool
14. Multi-tab support
15. Cookie management

### Faza 4: Advanced (5-7 dni)
16. Network interception
17. Performance metrics
18. PDF generation
19. Proxy support
20. Mobile emulation

---

## 🎯 Rekomendacja Priorytetów

**Natychmiast:**
- Usuń cleaner.rs (martwy kod)
- Dodaj error handling do stealth
- Fix race condition

**Ten tydzień:**
- Graceful shutdown
- Page persistence w state
- Cross-platform support

**Następny tydzień:**
- scroll, wait_for, execute_js tools
- Cookie management
- Multi-tab

---

## 💡 Podsumowanie

**Ocena:** 4.0/5 ⭐⭐⭐⭐
**Potencjał:** Bardzo wysoki - z ulepszeniami może być #1 browser MCP

**Główne wartości:**
- Stealth mode (unikalny)
- WAP-XML format (świetny dla LLM)
- Captcha integration (praktyczne)
- Lazy init (dobra architektura)

**Główne ryzyka:**
- Martwy kod
- Race conditions
- Brak graceful shutdown
- Tylko Windows

---

**Data analizy:** 2026-03-18
**Wersja:** 0.1.0
**Status:** ✅ Gotowy do ulepszeń
