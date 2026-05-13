/// Browser - jeden Chrome, dwa tryby ekstrakcji
///
/// ## Sposób działania
///
/// Zamiast osobnego reqwest + scraper, używamy **jednego Chrome** przez CDP.
/// Chrome ładuje stronę normalnie (JS się wykonuje, React/Vue renderuje).
/// Potem wstrzykujemy mały JS extractor który czyta **żywy DOM** i zwraca
/// nam WAP-style XML. Agent dostaje <2KB zamiast 500KB.
///
/// Ten sam `Page` od razu nadaje się do screenshota - żadnego duplikowania sesji.
///
/// ## Architektura 3-LLM
///   LLM1 (Planner)  → dostaje semantic XML → decyduje co kliknąć / wypełnić
///   LLM2 (Worker)   → wykonuje akcje przez MCP tools
///   LLM3 (Judge)    → dostaje screenshot base64 → weryfikuje wynik wizualnie
///
/// ## JS Extractor - serce systemu
///
/// JavaScript wstrzykiwany w żywy DOM wyciąga:
///   - <form> z method/action
///   - <input>, <textarea>, <select> z name/type/placeholder
///   - <button> i <input[type=submit]>
///   - <a href> - realne linki (bez #hash i javascript:)
///
/// Wynik to nasz WAP-XML z numerowanymi ID - identyczny format
/// niezależnie czy strona jest w HTML4, React, Angular czy Next.js.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chromiumoxide::browser::Browser;
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::page::ScreenshotParams;
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::types::PageState;

// ---------------------------------------------------------------------------
// Stealth JS - uruchamiany przed każdą akcją na stronie
// Maskuje wszystkie ślady headless Chrome / Chromedriver / CDP automation
// ---------------------------------------------------------------------------

/// Uruchamiamy to PRZED DOM_EXTRACTOR_JS i przed każdą akcją na stronie.
/// Bez tego każda strona z bot-detection zobaczy `navigator.webdriver = true`
/// i zaserwuje captcha albo odrzuci request.
///
/// Techniki używane przez Cloudflare, Akamai, DataDome, PerimeterX:
///   1. `navigator.webdriver` = true → od razu bot
///   2. Brak `navigator.plugins` → headless browser
///   3. `navigator.languages` = [] → headless
///   4. `window.chrome` = undefined → nie-Chrome w Chrome
///   5. CDP artifacts: `window.cdc_*`, `__webdriver_*` globals
const STEALTH_JS: &str = r#"
(function() {
    // 1. Usuń navigator.webdriver (najważniejsze - blokuje 90% detektorów)
    Object.defineProperty(navigator, 'webdriver', {
        get: () => undefined,
        configurable: true,
    });

    // 2. Udaj że mamy zainstalowane pluginy (headless = 0 pluginów)
    Object.defineProperty(navigator, 'plugins', {
        get: () => {
            const plugins = [
                { name: 'Chrome PDF Plugin', filename: 'internal-pdf-viewer',  description: 'Portable Document Format', length: 1 },
                { name: 'Chrome PDF Viewer',  filename: 'mhjfbmdgcfjbbpaeojofohoefgiehjai', description: '',                 length: 1 },
                { name: 'Native Client',      filename: 'internal-nacl-plugin',  description: '',                 length: 2 },
            ];
            plugins.__proto__ = PluginArray.prototype;
            return plugins;
        },
        configurable: true,
    });

    // 3. Ustaw normalne języki (headless = pusta tablica)
    Object.defineProperty(navigator, 'languages', {
        get: () => ['pl-PL', 'pl', 'en-US', 'en'],
        configurable: true,
    });

    // 4. Przywróć window.chrome (headless go nie ma)
    if (!window.chrome) {
        Object.defineProperty(window, 'chrome', {
            value: {
                app: { isInstalled: false, InstallState: {}, RunningState: {} },
                runtime: {
                    OnInstalledReason: {},
                    OnRestartRequiredReason: {},
                    PlatformArch: {},
                    PlatformNaclArch: {},
                    PlatformOs: {},
                    RequestUpdateCheckStatus: {},
                    connect: function() {},
                    sendMessage: function() {},
                },
                loadTimes: function() {},
                csi: function() {},
            },
            writable: false,
            configurable: false,
        });
    }

    // 5. Usuń CDP/Chromedriver artifacts
    const cdcKeys = Object.keys(window).filter(k =>
        k.startsWith('cdc_') || k.startsWith('__webdriver') || k.startsWith('__driver')
    );
    cdcKeys.forEach(k => { try { delete window[k]; } catch(_) {} });

    // 6. Popraw permissions API (headless zwraca 'denied' dla notifications)
    const origQuery = window.Permissions && window.Permissions.prototype.query;
    if (origQuery) {
        window.Permissions.prototype.query = function(parameters) {
            if (parameters.name === 'notifications') {
                return Promise.resolve({ state: Notification.permission });
            }
            return origQuery.apply(this, [parameters]);
        };
    }

    // 7. Ustaw realistyczny screen (headless = 0x0 lub dziwne wymiary)
    Object.defineProperty(screen, 'availWidth',  { get: () => 1920 });
    Object.defineProperty(screen, 'availHeight', { get: () => 1040 });
    Object.defineProperty(screen, 'width',       { get: () => 1920 });
    Object.defineProperty(screen, 'height',      { get: () => 1080 });
    Object.defineProperty(screen, 'colorDepth',  { get: () => 24 });
    Object.defineProperty(screen, 'pixelDepth',  { get: () => 24 });
})();
true
"#;

// ---------------------------------------------------------------------------
// JS Extractor - wstrzykiwany w Chrome DOM
// ---------------------------------------------------------------------------

/// JavaScript który wstrzykujemy w załadowaną stronę.
/// Chodzi po żywym DOM i zwraca nasz WAP-XML jako string.
/// Działa na każdej stronie - statycznej i SPA (React/Vue/Angular).
const DOM_EXTRACTOR_JS: &str = r#"
(function() {
    let id = 1;
    const lines = [];
    const url = window.location.href;
    const title = document.title || '';

    // --- Helpers ---

    function esc(s) {
        return String(s || '')
            .replace(/&/g, '&amp;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;')
            .replace(/"/g, '&quot;');
    }

    function abs(href) {
        try { return new URL(href, window.location.href).href; }
        catch(e) { return href; }
    }

    function text(el) {
        return (el.innerText || el.textContent || '').replace(/\s+/g, ' ').trim();
    }

    // Czy element jest "śmieciem" - nav, reklamy, stopki, skrypty
    function isNoise(el) {
        const tag = el.tagName && el.tagName.toLowerCase();
        if (['script','style','noscript','iframe','svg','canvas',
             'nav','header','footer','aside'].includes(tag)) return true;
        const role = (el.getAttribute && el.getAttribute('role') || '').toLowerCase();
        if (['navigation','banner','contentinfo','complementary'].includes(role)) return true;
        const cls = (el.className && typeof el.className === 'string' ? el.className : '').toLowerCase();
        const nid = (el.id || '').toLowerCase();
        // Typowe klasy/id reklam i nawigacji
        const noiseWords = ['nav','menu','sidebar','footer','header','cookie','popup',
                            'modal','banner','advert','promo','social','share','comment',
                            'widget','breadcrumb','pagination','related','newsletter'];
        return noiseWords.some(w => cls.includes(w) || nid.includes(w));
    }

    // Znajdź główny obszar treści (Reader Mode heuristic)
    function findMainContent() {
        // Kolejność priorytetów - od najbardziej semantycznych
        const candidates = [
            document.querySelector('article'),
            document.querySelector('main'),
            document.querySelector('[role="main"]'),
            document.querySelector('#content'),
            document.querySelector('#main'),
            document.querySelector('.content'),
            document.querySelector('.post'),
            document.querySelector('.article'),
            document.querySelector('.entry'),
            document.querySelector('.page-content'),
            document.body
        ];
        return candidates.find(el => el !== null) || document.body;
    }

    lines.push(`<page url="${esc(url)}" title="${esc(title)}">`);

    // =========================================================
    // WARSTWA 1: TREŚĆ (Reader Mode - WAP/RSS style)
    // Nagłówki + akapity z głównego obszaru treści.
    // Pomijamy nav/footer/reklamy.
    // =========================================================

    const mainContent = findMainContent();
    const seenText = new Set(); // deduplikacja identycznych akapitów
    const CONTENT_CHAR_LIMIT = 8000; // max chars treci (bez WAP tagów)
    let contentChars = 0;
    let contentTruncated = false;

    // Przechodzimy po węzłach w kolejności DOM
    const walker = document.createTreeWalker(
        mainContent,
        NodeFilter.SHOW_ELEMENT,
        {
            acceptNode: function(node) {
                if (isNoise(node)) return NodeFilter.FILTER_REJECT; // pomiń całe poddrzewo
                return NodeFilter.FILTER_ACCEPT;
            }
        }
    );

    let node = walker.nextNode();
    while (node && !contentTruncated) {
        const tag = node.tagName.toLowerCase();

        // Nagłówki
        if (['h1','h2','h3','h4'].includes(tag)) {
            const t = text(node);
            if (t && t.length > 1 && !seenText.has(t)) {
                seenText.add(t);
                lines.push(`  <${tag}>${esc(t)}</${tag}>`);
                contentChars += t.length;
            }
        }

        // Akapity - tylko bezpośrednia treść (nie zagnieżdżone p w p)
        if (tag === 'p' || tag === 'li') {
            const t = text(node);
            // Filtruj: min 20 znaków, nie duplikaty, nie same linki
            if (t && t.length >= 20 && !seenText.has(t)) {
                seenText.add(t);
                lines.push(`  <p>${esc(t)}</p>`);
                contentChars += t.length;
                if (contentChars >= CONTENT_CHAR_LIMIT) {
                    lines.push(`  <truncated max_chars="${CONTENT_CHAR_LIMIT}" note="use scroll or get_full_text for more"/>`);
                    contentTruncated = true;
                }
            }
        }

        node = walker.nextNode();
    }

    // =========================================================
    // WARSTWA 2: INTERAKCJA (formularze, linki, przyciski)
    // Agent wie co może kliknąć / wypełnić
    // =========================================================

    // --- Formularze i ich pola (max 5) ---
    let forms = Array.from(document.querySelectorAll('form')).slice(0, 5);
    forms.forEach(form => {
        const fid = id++;
        const method = (form.method || 'GET').toUpperCase();
        const action = abs(form.action || form.getAttribute('action') || '');
        lines.push(`  <form id="${fid}" method="${method}" action="${esc(action)}">`);

        form.querySelectorAll('input:not([type=hidden]), textarea, select').forEach(inp => {
            const iid = id++;
            const itype = (inp.type || 'text').toLowerCase();
            const name = esc(inp.name || '');
            const labelEl = inp.id ? document.querySelector(`label[for="${inp.id}"]`) : null;
            const placeholder = esc(
                inp.placeholder ||
                (labelEl ? text(labelEl) : '') ||
                inp.getAttribute('aria-label') ||
                name
            );
            if (inp.tagName === 'SELECT') {
                lines.push(`    <select id="${iid}" name="${name}" />`);
            } else {
                lines.push(`    <input id="${iid}" name="${name}" type="${itype}" placeholder="${placeholder}" />`);
            }
        });

        form.querySelectorAll('button, input[type=submit], input[type=button]').forEach(btn => {
            const bid = id++;
            const label = esc(text(btn) || btn.value || 'Submit');
            lines.push(`    <button id="${bid}">${label}</button>`);
        });

        lines.push(`  </form>`);
    });

    // --- Linki (poza formularzami, bez #hash i javascript:) max 100 ---
    // Deduplikujemy po href zeby nie listowac 50x tego samego menu
    const seenHrefs = new Set();
    const MAX_LINKS = 100;
    let linkCount = 0;
    for (const a of document.querySelectorAll('a[href]')) {
        if (linkCount >= MAX_LINKS) {
            lines.push(`  <links_truncated total_visible="${MAX_LINKS}" note="use get_links for full list"/>`);
            break;
        }
        if (a.closest('form')) continue;
        const href = a.getAttribute('href') || '';
        if (!href || href.startsWith('#') || href.startsWith('javascript:')) continue;
        const resolved = abs(href);
        if (seenHrefs.has(resolved)) continue;
        seenHrefs.add(resolved);
        const label = esc(text(a) || a.getAttribute('aria-label') || href);
        if (!label) continue;
        lines.push(`  <link id="${id++}" href="${esc(resolved)}">${label}</link>`);
        linkCount++;
    }

    // --- Luźne przyciski (poza formularzami) ---
    document.querySelectorAll('button, input[type=submit]').forEach(btn => {
        if (!btn.closest('form')) {
            const label = esc(text(btn) || btn.value || 'Button');
            lines.push(`  <button id="${id++}">${label}</button>`);
        }
    });

    lines.push(`</page>`);
    return lines.join('\n');
})()
"#;

// ---------------------------------------------------------------------------
// ChromeBrowser - jeden instance Chrome dla wszystkich operacji
// ---------------------------------------------------------------------------

pub struct ChromeBrowser {
    browser: Arc<Mutex<Browser>>,
    _handler: tokio::task::JoinHandle<()>,
    // Uchwyt do procesu Chrome - trzymamy żeby go zamknąć przy shutdown
    _chrome_process: Option<std::sync::Mutex<std::process::Child>>,
}

impl ChromeBrowser {
    /// Startuje Chrome headless.
    /// Na Windows Chrome jest zazwyczaj w:
    ///   C:\Program Files\Google\Chrome\Application\chrome.exe
    pub async fn launch() -> Result<Self> {
        info!("Launching Chrome headless via CDP...");

        // Znajdź Chrome na Windows
        let chrome_exe = Self::find_chrome()?;
        info!("Chrome found: {}", chrome_exe);

        // UA bez "HeadlessChrome" - boty to pierwsze sprawdzają
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
                  AppleWebKit/537.36 (KHTML, like Gecko) \
                  Chrome/120.0.0.0 Safari/537.36";

        // Świeży temp profile - isolacja, nie koliduje z uruchomionym Chrome
        let temp_dir = std::env::temp_dir().join("rust_browser_mcp_profile");
        // Usuń cały stary profil żeby nie blokował - "Multiple targets not supported in headless mode"
        // (poprzednie procesy Chrome mogły zostawić SingletonLock + Singleton*)
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);

        // Uruchamiamy Chrome sami - pełna kontrola nad flagami
        // Znajdź wolny port - bind na 0 żeby OS przydzielił port, potem zwolnij
        let free_port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0")
                .context("Nie można znaleźć wolnego portu")?;
            listener.local_addr()?.port()
        };
        let port_arg = format!("--remote-debugging-port={}", free_port);
        let ua_arg = format!("--user-agent={}", ua);
        let udir_arg = format!("--user-data-dir={}", temp_dir.display());

        info!("Chrome CDP port: {}", free_port);

        let mut child = std::process::Command::new(&chrome_exe)
            .args([
                "--headless=new",
                "--no-sandbox",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-first-run-ui",
                // Stealth
                "--disable-blink-features=AutomationControlled",
                &ua_arg,
                "--lang=pl-PL,pl",
                "--accept-lang=pl-PL,pl,en-US,en",
                "--disable-infobars",
                // Dev
                "--ignore-certificate-errors",
                "--disable-web-security",
                &port_arg,
                &udir_arg,
            ])
            .stdin(std::process::Stdio::null())   // NIE dziedzicz stdin MCP pipe - Chrome by exiował!
            .stderr(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
            .context("Nie można uruchomić Chrome. Upewnij się że jest zainstalowany.")?;

        // Poczekaj aż Chrome uruchomi HTTP API: GET http://127.0.0.1:PORT/json/version
        // Bardziej niezawodne niż parsowanie stderr - nie zależy od formatu logów
        let poll_result = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            Self::poll_devtools_url(free_port),
        )
        .await;

        // Jeśli poll się nie powiódł, zabij Chrome process żeby nie był zombie
        if poll_result.is_err() || poll_result.as_ref().ok().map(|o| o.is_none()).unwrap_or(false) {
            let _ = child.kill();
        }

        let ws_url = poll_result
            .context("Timeout: Chrome nie odpowiedział na /json/version w 15s")?
            .context("Chrome nie uruchomił DevTools HTTP API")?;

        info!("Chrome DevTools WS: {}", ws_url);

        // Podłącz chromiumoxide do działającego Chrome przez CDP
        let (browser, mut handler) = Browser::connect(ws_url)
            .await
            .context("Nie można podłączyć do Chrome CDP")?;

        // Zachowaj uchwyt do child process żeby nie był zabity po dropie
        let chrome_child = child;

        // Pompuj CDP handler w tle
        let handle = tokio::spawn(async move {
            loop {
                if handler.next().await.is_none() {
                    break;
                }
            }
        });

        info!("Chrome headless ready (connect_over_cdp)");
        Ok(Self {
            browser: Arc::new(Mutex::new(browser)),
            _handler: handle,
            _chrome_process: Some(std::sync::Mutex::new(chrome_child)),
        })
    }

    /// Szuka exe Chrome w standardowych lokalizacjach Windows
    fn find_chrome() -> Result<String> {
        // Najpierw env var CHROME_EXE
        if let Ok(p) = std::env::var("CHROME_EXE") {
            if std::path::Path::new(&p).exists() {
                return Ok(p);
            }
        }
        let candidates = [
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
            r"C:\Program Files (x86)\BraveSoftware\Brave-Browser\Application\brave.exe",
        ];
        for c in &candidates {
            if std::path::Path::new(c).exists() {
                return Ok(c.to_string());
            }
        }
        // Próba przez LOCALAPPDATA
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let p = format!("{}\\Google\\Chrome\\Application\\chrome.exe", local);
            if std::path::Path::new(&p).exists() {
                return Ok(p);
            }
        }
        anyhow::bail!(
            "Chrome/Brave nie znaleziony. Ustaw env CHROME_EXE=<ścieżka> lub zainstaluj Chrome."
        )
    }

    /// Pollinguje Chrome DevTools HTTP API aż odpowie, zwraca webSocketDebuggerUrl.
    /// Niezawodne na wszystkich platformach - nie zależy od parsowania stderr.
    async fn poll_devtools_url(port: u16) -> Option<String> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let addr = format!("127.0.0.1:{}", port);
        let request = format!(
            "GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            port
        );
        // Próbuj co 200ms przez max 15s (75 prób)
        for attempt in 0u32..75 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let Ok(mut stream) = tokio::net::TcpStream::connect(&addr).await else {
                continue;
            };
            if stream.write_all(request.as_bytes()).await.is_err() {
                continue;
            }
            let mut buf = Vec::with_capacity(4096);
            // Chrome (HTTP/1.1) nie zawsze zamyka połączenie — read_to_end by zawisło.
            // Czekamy max 500ms na dane, potem parsujemy co dostaliśmy.
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                stream.read_to_end(&mut buf),
            )
            .await;
            let body = std::str::from_utf8(&buf).unwrap_or("");
            // HTTP response: headers\r\n\r\nbody
            let json_str = if let Some(pos) = body.find("\r\n\r\n") {
                &body[pos + 4..]
            } else {
                body
            };
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(json_str) {
                if let Some(ws) = json.get("webSocketDebuggerUrl").and_then(|v| v.as_str()) {
                    info!("Chrome DevTools WS (attempt {}): {}", attempt, ws);
                    return Some(ws.to_string());
                }
            }
        }
        None
    }

    /// Nawiguje do URL, czeka na załadowanie (JS też), wstrzykuje extractor.
    /// Zwraca semantic WAP-XML z numerowanymi ID.
    /// Aktualizuje PageState: url, title, (elements są w XML bo JS je liczy).
    pub async fn navigate_wap(&self, url: &str, state: &mut PageState) -> Result<String> {
        info!("Chrome navigate → WAP extraction: {}", url);
        state.reset(url);

        let browser = self.browser.lock().await;
        let page = browser
            .new_page(url)
            .await
            .context("failed to open page")?;

        // Czekaj na network idle - JS frameworks muszą się wyrenderować
        page.wait_for_navigation().await
            .context("page load timeout")?;

        // Maskuj headless fingerprint PRZED każdą akcją na stronie
        // (navigator.webdriver, plugins, languages, window.chrome, CDP artifacts)
        // CRITICAL: Sprawdzamy wynik - jeśli stealth się nie wstrzyknie, bot detection nas złapie
        match page.evaluate(STEALTH_JS).await {
            Ok(result) => {
                let success: bool = result.into_value().unwrap_or(false);
                if !success {
                    warn!("STEALTH_JS returned false - bot detection may trigger!");
                }
            }
            Err(e) => {
                warn!("STEALTH_JS injection failed: {} - retrying...", e);
                // Retry once
                if let Err(e2) = page.evaluate(STEALTH_JS).await {
                    error!("STEALTH_JS retry failed: {} - page may be detected as bot!", e2);
                }
            }
        }

        // Wstrzyknij JS extractor - czyta żywy DOM, zwraca WAP-XML string
        let xml: String = page
            .evaluate(DOM_EXTRACTOR_JS)
            .await
            .context("JS extractor failed")?
            .into_value()
            .context("JS extractor returned non-string")?;

        // Wyciągnij aktualny URL (po redirect) i tytuł
        let final_url: String = page
            .evaluate("window.location.href")
            .await
            .ok()
            .and_then(|v| v.into_value().ok())
            .unwrap_or_else(|| url.to_string());

        let title: String = page
            .evaluate("document.title")
            .await
            .ok()
            .and_then(|v| v.into_value().ok())
            .unwrap_or_default();

        state.url = final_url;
        state.title = title;
        // Zapamiętaj ID strony (page handle) żeby możliwe było fill/submit bez nowego requesta
        // W uproszczeniu zamykamy zakładkę - dla formularzy otworzymy nową
        let _ = page.close().await;

        info!("WAP extraction done: {} chars XML", xml.len());
        Ok(xml)
    }

    /// Wypełnia pole formularza przez CDP na załadowanej stronie
    /// i submituje - zwraca WAP-XML strony wynikowej.
    ///
    /// `fields`: Vec<(name, value)> - wartości do wpisania
    /// `form_selector`: CSS selector formularza (np. "form" lub "form#login")
    pub async fn fill_and_submit(
        &self,
        url: &str,
        fields: &[(String, String)],
        form_selector: &str,
        state: &mut PageState,
    ) -> Result<String> {
        info!("Chrome fill_and_submit: {} fields on {}", fields.len(), url);
        state.reset(url);

        let browser = self.browser.lock().await;
        let page = browser.new_page(url).await?;
        page.wait_for_navigation().await?;

        // Maskuj headless fingerprint przed wypełnianiem formularza
        let _ = page.evaluate(STEALTH_JS).await;

        // Wpisz wartości przez JS
        for (name, value) in fields {
            let js = format!(
                r#"(function(){{
                    var el = document.querySelector('[name="{name}"]');
                    if(el){{
                        var nativeInput = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value');
                        if(nativeInput && nativeInput.set) nativeInput.set.call(el, '{value}');
                        else el.value = '{value}';
                        el.dispatchEvent(new Event('input', {{bubbles:true}}));
                        el.dispatchEvent(new Event('change', {{bubbles:true}}));
                    }}
                }})();"#,
                name = name.replace('"', "\\\""),
                value = value.replace('"', "\\\"").replace('\n', "\\n")
            );
            page.evaluate(js).await.ok();
        }

        // Submituj formularz - kliknij button (nie form.submit() - omija JS handlery SPA)
        let submit_js = format!(
            r#"(function(){{
                var form = document.querySelector('{sel}');
                if(!form) return;
                // Sprobuj kliknac submit button - React/SPA respektuje to
                var btn = form.querySelector('[type=submit], button:not([type=button])');
                if(btn) {{ btn.click(); return; }}
                // Fallback: Enter na aktywnym inpucie
                var inp = form.querySelector('input:not([type=hidden])');
                if(inp) {{
                    inp.dispatchEvent(new KeyboardEvent('keypress', {{key:'Enter',keyCode:13,bubbles:true}}));
                    inp.dispatchEvent(new KeyboardEvent('keydown',  {{key:'Enter',keyCode:13,bubbles:true}}));
                    inp.dispatchEvent(new KeyboardEvent('keyup',    {{key:'Enter',keyCode:13,bubbles:true}}));
                    return;
                }}
                form.submit();
            }})();"#,
            sel = form_selector.replace('\'', "\\'")
        );
        page.evaluate(submit_js).await.ok();
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        page.wait_for_navigation().await.ok();

        // Wyciągnij wynikowy WAP-XML
        let xml: String = page
            .evaluate(DOM_EXTRACTOR_JS)
            .await?
            .into_value()
            .context("extractor returned non-string after submit")?;

        let final_url: String = page
            .evaluate("window.location.href")
            .await
            .ok()
            .and_then(|v| v.into_value().ok())
            .unwrap_or_else(|| url.to_string());

        state.url = final_url;
        let _ = page.close().await;
        Ok(xml)
    }

    /// Screenshot przez CDP - PNG jako base64.
    /// Ten sam Chrome co obsługuje WAP - zero duplikacji.
    pub async fn screenshot(&self, url: &str) -> Result<String> {
        info!("Chrome screenshot: {}", url);
        let browser = self.browser.lock().await;
        let page = browser.new_page(url).await?;
        page.wait_for_navigation().await?;
        let _ = page.evaluate(STEALTH_JS).await;

        let params = ScreenshotParams::builder()
            .format(CaptureScreenshotFormat::Png)
            .build(); // viewport only - full_page=true generuje 3-4MB PNG (za duze dla LLM)

        let png: Vec<u8> = page
            .screenshot(params)
            .await
            .context("screenshot failed")?;

        let _ = page.close().await;
        let b64 = B64.encode(&png);
        info!("Screenshot: {} bytes → {} b64 chars", png.len(), b64.len());
        Ok(b64)
    }

    /// Wstrzykuje dowolny JS na aktualnie załadowanej stronie i zwraca String
    pub async fn evaluate_on_url(&self, url: &str, js: &str) -> Result<String> {
        let browser = self.browser.lock().await;
        let page = browser.new_page(url).await?;
        page.wait_for_navigation().await?;
        let _ = page.evaluate(STEALTH_JS).await;
        let result: String = page
            .evaluate(js)
            .await?
            .into_value()
            .context("JS returned non-string")?;
        let _ = page.close().await;
        Ok(result)
    }

    /// Jak evaluate_on_url ale zwraca Option<String> (null → None)
    pub async fn evaluate_on_url_opt(&self, url: &str, js: &str) -> Result<Option<String>> {
        let browser = self.browser.lock().await;
        let page = browser.new_page(url).await?;
        page.wait_for_navigation().await?;
        let _ = page.evaluate(STEALTH_JS).await;
        let val = page.evaluate(js).await?;
        let _ = page.close().await;
        Ok(val.into_value().ok())
    }

    /// Klika element WAP-XML o podanym ID (button, link, input[submit]).
    /// Otwiera aktualną stronę (ze state), klika element, czeka na nawigację/re-render,
    /// zwraca WAP-XML nowej/zaktualizowanej strony.
    pub async fn click_wap_element(&self, wap_id: u32, state: &mut PageState) -> Result<String> {
        let url = state.url.clone();
        info!("Chrome click_wap_element: id={} on {}", wap_id, url);

        if url.is_empty() {
            return Err(anyhow::anyhow!("Brak URL - użyj navigate() najpierw"));
        }

        let browser = self.browser.lock().await;
        let page = browser.new_page(&url).await?;
        page.wait_for_navigation().await?;
        let _ = page.evaluate(STEALTH_JS).await;

        // JS który replikuje DOKŁADNIE ten sam counter co DOM_EXTRACTOR_JS
        // i klika element o podanym WAP id
        let click_js = format!(r#"
(function() {{
    const targetId = {target_id};
    let n = 1;

    // 1. Formularze: id formy, potem inputy, potem buttony
    for (const form of document.querySelectorAll('form')) {{
        n++; // id samego formularza
        for (const _inp of form.querySelectorAll('input:not([type=hidden]), textarea, select')) {{
            n++; // id każdego inputu - nie klikamy
        }}
        for (const btn of form.querySelectorAll('button, input[type=submit], input[type=button]')) {{
            if (n === targetId) {{
                btn.scrollIntoView({{block:'center'}});
                btn.click();
                return 'button_in_form';
            }}
            n++;
        }}
    }}

    // 2. Linki poza formularzami (deduplikacja po href - identyczna jak DOM_EXTRACTOR_JS)
    const seenHrefs = new Set();
    for (const a of document.querySelectorAll('a[href]')) {{
        if (a.closest('form')) continue;
        const href = a.getAttribute('href') || '';
        if (!href || href.startsWith('#') || href.startsWith('javascript:')) continue;
        let resolved;
        try {{ resolved = new URL(href, window.location.href).href; }} catch(e) {{ resolved = href; }}
        if (seenHrefs.has(resolved)) continue;
        seenHrefs.add(resolved);
        const label = (a.innerText||a.textContent||'').replace(/\\s+/g,' ').trim()||a.getAttribute('aria-label')||href;
        if (!label) continue;
        if (n === targetId) {{
            a.scrollIntoView({{block:'center'}});
            a.click();
            return 'link';
        }}
        n++;
    }}

    // 3. Luźne buttony poza formularzami
    for (const btn of document.querySelectorAll('button, input[type=submit]')) {{
        if (btn.closest('form')) continue;
        if (n === targetId) {{
            btn.scrollIntoView({{block:'center'}});
            btn.click();
            return 'loose_button';
        }}
        n++;
    }}

    return null;
}})()
"#, target_id = wap_id);

        let click_result: Option<String> = page
            .evaluate(click_js)
            .await
            .ok()
            .and_then(|v| v.into_value().ok());

        if click_result.is_none() || click_result.as_deref() == Some("null") {
            let _ = page.close().await;
            return Err(anyhow::anyhow!("Element WAP id={} nie znaleziony na stronie", wap_id));
        }
        info!("Click result: {:?}", click_result);

        // Czekaj na potencjalną nawigację (max 2s) + 300ms na React/Vue re-render
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(2000),
            page.wait_for_navigation(),
        ).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        // Nowy URL po kliku
        let final_url: String = page
            .evaluate("window.location.href")
            .await
            .ok()
            .and_then(|v| v.into_value().ok())
            .unwrap_or_else(|| url.clone());

        // Stealth + DOM extraction na nowej/zaktualizowanej stronie
        let _ = page.evaluate(STEALTH_JS).await;
        let xml: String = page
            .evaluate(DOM_EXTRACTOR_JS)
            .await
            .ok()
            .and_then(|v| v.into_value().ok())
            .unwrap_or_else(|| format!("<page url=\"{}\" title=\"\"></page>", final_url));

        let title: String = page
            .evaluate("document.title")
            .await
            .ok()
            .and_then(|v| v.into_value().ok())
            .unwrap_or_default();

        state.url = final_url;
        state.title = title;
        let _ = page.close().await;

        info!("click_wap_element done: {} chars XML", xml.len());
        Ok(xml)
    }

    /// Przewija stronę o podaną liczbę pikseli w dół, czeka na lazy-load content (500ms),
    /// zwraca odświeżony WAP-XML. Przydatne do infinite scroll i stron z lazyload.
    pub async fn scroll_and_extract(&self, url: &str, pixels: u32, state: &mut PageState) -> Result<String> {
        info!("Chrome scroll_and_extract: {} px on {}", pixels, url);
        let browser = self.browser.lock().await;
        let page = browser.new_page(url).await?;
        page.wait_for_navigation().await?;
        let _ = page.evaluate(STEALTH_JS).await;

        let scroll_js = format!("window.scrollBy(0, {}); true", pixels);
        let _ = page.evaluate(scroll_js).await;

        // Czekaj na lazy-load content
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;

        let xml: String = page
            .evaluate(DOM_EXTRACTOR_JS)
            .await?
            .into_value()
            .context("DOM extraction failed after scroll")?;

        let final_url: String = page
            .evaluate("window.location.href")
            .await
            .ok()
            .and_then(|v| v.into_value().ok())
            .unwrap_or_else(|| url.to_string());

        let title: String = page
            .evaluate("document.title")
            .await
            .ok()
            .and_then(|v| v.into_value().ok())
            .unwrap_or_default();

        state.url = final_url;
        state.title = title;
        let _ = page.close().await;
        Ok(xml)
    }

    /// Wykonuje dowolny JS na aktualnej stronie, zwraca wynik jako string.
    /// Escape hatch gdy WAP-XML nie wystarczy. Stealth aktywny.
    pub async fn execute_js_raw(&self, url: &str, js_code: &str) -> Result<String> {
        info!("Chrome execute_js: {} chars on {}", js_code.len(), url);
        let browser = self.browser.lock().await;
        let page = browser.new_page(url).await?;
        page.wait_for_navigation().await?;
        let _ = page.evaluate(STEALTH_JS).await;

        let result: serde_json::Value = page
            .evaluate(js_code)
            .await
            .context("JS execution failed")?
            .into_value()
            .unwrap_or(serde_json::Value::Null);

        let _ = page.close().await;

        let output = match &result {
            serde_json::Value::String(s) => s.clone(),
            other => serde_json::to_string_pretty(other)
                .unwrap_or_else(|_| "null".to_string()),
        };
        Ok(output)
    }

    /// Wyciąga tabele (lub wg CSS selectora) jako JSON array.
    /// Każda tabela: { table_index, caption, headers, rows, row_count, col_count }
    pub async fn extract_tables(&self, url: &str, selector: Option<&str>) -> Result<String> {
        info!("Chrome extract_tables: selector={:?} on {}", selector, url);
        let sel = selector.unwrap_or("table").replace('\'', "\\'");
        let table_js = format!(r#"
(function() {{
    const tables = document.querySelectorAll('{sel}');
    const result = [];
    tables.forEach((table, ti) => {{
        const headers = [];
        const thEls = table.querySelectorAll('thead th, thead td, tr:first-child th');
        thEls.forEach(th => headers.push((th.innerText||th.textContent||'').replace(/\s+/g,' ').trim()));
        const rows = [];
        table.querySelectorAll('tbody tr, tr').forEach(tr => {{
            const cells = [];
            tr.querySelectorAll('td, th').forEach(td => {{
                cells.push((td.innerText||td.textContent||'').replace(/\s+/g,' ').trim());
            }});
            if (cells.length > 0) rows.push(cells);
        }});
        const captionEl = table.querySelector('caption');
        result.push({{
            table_index: ti,
            caption: captionEl ? (captionEl.innerText||'').trim() : '',
            headers: headers,
            rows: rows,
            row_count: rows.length,
            col_count: headers.length || (rows[0] ? rows[0].length : 0)
        }});
    }});
    return JSON.stringify(result, null, 2);
}})()
"#, sel = sel);

        let browser = self.browser.lock().await;
        let page = browser.new_page(url).await?;
        page.wait_for_navigation().await?;
        let _ = page.evaluate(STEALTH_JS).await;

        let result: String = page
            .evaluate(table_js)
            .await?
            .into_value()
            .context("table extraction failed")?;

        let _ = page.close().await;
        Ok(result)
    }

    /// Screenshot jako VerifyResult (do wysłania do Judge LLM)
    #[allow(dead_code)]
    pub async fn screenshot_current_as_result(&self, url: &str) -> Result<VerifyResult> {
        let b64 = self.screenshot(url).await?;
        Ok(VerifyResult {
            url: url.to_string(),
            screenshot_base64: b64,
            mime: "image/png".to_string(),
        })
    }
}

/// Wynik weryfikacji wizualnej - przekazywany do Judge LLM przez MCP
#[derive(Debug, serde::Serialize)]
pub struct VerifyResult {
    pub url: String,
    /// PNG zakodowany base64 - wrzucasz bezpośrednio do {type: "image", data: "..."} w prompt
    pub screenshot_base64: String,
    pub mime: String,
}

// ---------------------------------------------------------------------------
// Graceful shutdown - zabij Chrome process przy dropie
// ---------------------------------------------------------------------------

impl Drop for ChromeBrowser {
    fn drop(&mut self) {
        info!("Shutting down Chrome browser...");

        // Zabij Chrome process jeśli żyje
        if let Some(ref chrome_mutex) = self._chrome_process {
            if let Ok(mut child) = chrome_mutex.lock() {
                match child.kill() {
                    Ok(_) => info!("Chrome process killed successfully"),
                    Err(e) => warn!("Failed to kill Chrome process: {}", e),
                }
                // Poczekaj na zakończenie procesu (max 5s)
                match child.wait() {
                    Ok(status) => info!("Chrome process exited with: {}", status),
                    Err(e) => warn!("Chrome process wait failed: {}", e),
                }
            }
        }

        // Abort CDP handler task
        self._handler.abort();

        info!("Chrome browser shutdown complete");
    }
}

// ---------------------------------------------------------------------------
// AgentBrowser - fasada z session state
// ---------------------------------------------------------------------------

pub struct AgentBrowser {
    pub chrome: ChromeBrowser,
    pub state: PageState,
    /// Zapamiętane wartości inputów: name → value (do fill_and_submit)
    pub pending_inputs: Vec<(String, String)>,
}

impl AgentBrowser {
    pub async fn new() -> Result<Self> {
        let chrome = match ChromeBrowser::launch().await {
            Ok(c) => c,
            Err(e) => {
                warn!("Chrome launch failed: {}. Retrying without sandbox...", e);
                // Drugi try - czasem potrzebny na CI/serwerach
                ChromeBrowser::launch().await?
            }
        };
        Ok(Self {
            chrome,
            state: PageState::new(),
            pending_inputs: Vec::new(),
        })
    }

    pub fn add_input(&mut self, name: &str, value: &str) {
        // Nadpisz jeśli już istnieje
        if let Some(entry) = self.pending_inputs.iter_mut().find(|(n, _)| n == name) {
            entry.1 = value.to_string();
        } else {
            self.pending_inputs.push((name.to_string(), value.to_string()));
        }
    }

    pub fn clear_inputs(&mut self) {
        self.pending_inputs.clear();
    }
}
