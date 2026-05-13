/// MCP Server - JSON-RPC 2.0 over stdio
///
/// Protokół MCP (Model Context Protocol) od Anthropic:
/// - Transport: stdin/stdout (newline-delimited JSON)
/// - Inicjalizacja: initialize → initialized
/// - Narzędzia: tools/list → tools/call
///
/// Wystawione tools:
///   navigate(url)              → semantic XML strony (tryb WAP, szybki)
///   get_content()              → semantic XML aktualnej strony (z cache)
///   click_link(id)             → navigate do href elementu o danym id
///   fill_input(name, value)    → ustaw wartość pola formularza
///   submit_form(form_id)       → wyślij formularz POST/GET
///   screenshot(url?)           → PNG base64 przez Chrome CDP (tryb Vision)
///   get_state()                → bieżący URL, tytuł strony

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, OnceCell};
use tracing::{debug, error, info, warn};

use crate::browser::AgentBrowser;
use crate::captcha::{CaptchaConfig, CaptchaSolver, CaptchaTask, CaptchaService};

// ---------------------------------------------------------------------------
// Typy JSON-RPC 2.0
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Request {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct Response {
    jsonrpc: String,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

impl Response {
    fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn err(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Definicje narzędzi MCP (dla tools/list)
// ---------------------------------------------------------------------------

fn tool_definitions() -> Value {
    json!({
        "tools": [
            {
                "name": "navigate",
                "description": "Otwiera URL w Chrome (JS się wykonuje). Zwraca WAP-XML: tytuł + treść (h1/h2/p bez śmieci) + formularze + linki z ID. Jak RSS/Reader Mode + mapa akcji w jednym.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Pełny URL do otwarcia (np. https://example.com)"
                        }
                    },
                    "required": ["url"]
                }
            },
            {
                "name": "get_text",
                "description": "Zwraca TYLKO czysty tekst aktualnej strony (nagłówki + akapity, zero tagów XML, zero linków). Jak artykuł RSS. Idealny gdy LLM ma przeczytać/przeanalizować treść strony.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "get_links",
                "description": "Zwraca TYLKO listę linków aktualnej strony (id, href, label). Bez treści. Idealny do nawigacji i odkrywania struktury serwisu.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "get_content",
                "description": "Zwraca pełny WAP-XML aktualnej strony (treść + formularze + linki). Ten sam format co navigate() ale bez nowego requesta.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "click_link",
                "description": "Nawiguje do linku o podanym ID (z WAP-XML). Klika <link id='X'>.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "integer",
                            "description": "ID elementu link z WAP-XML"
                        }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "click_button",
                "description": "Klika button/przycisk/link na stronie według ID z WAP-XML. Działa na <button id='X'> i <link id='X'> - zarówno wewnątrz formularzy jak i poza nimi. Czeka na nawigację lub re-render i zwraca nowy WAP-XML.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "integer",
                            "description": "ID elementu z WAP-XML (np. <button id='44'> → podaj 44)"
                        }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "fill_and_submit",
                "description": "Wypełnia pola formularza i wysyła go. Podaj url strony, pola jako obiekt name→value, i CSS selector formularza. Zwraca WAP-XML strony wynikowej.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "URL strony z formularzem"
                        },
                        "fields": {
                            "type": "object",
                            "description": "Mapa name→value pól do wypełnienia, np. {\"email\": \"jan@example.com\", \"password\": \"tajne\"}"
                        },
                        "form_selector": {
                            "type": "string",
                            "description": "CSS selector formularza, np. 'form' lub 'form#login' lub 'form.registration' (domyślnie: 'form')"
                        }
                    },
                    "required": ["url", "fields"]
                }
            },
            {
                "name": "screenshot",
                "description": "Robi screenshot przez Chrome (pełna strona). Zwraca PNG jako base64. Używaj do weryfikacji: czy formularz się wysłał, czy wyskoczył captcha, czy strona wygląda poprawnie.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "URL do screenshota (opcjonalny - domyślnie aktualny URL z navigate)"
                        }
                    }
                }
            },
            {
                "name": "get_state",
                "description": "Zwraca aktualny URL i tytuł strony.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "solve_captcha",
                "description": "Rozwiązuje captchę przez zewnętrzny serwis (2captcha/anti-captcha/CapMonster). Obsługuje: image (obrazek z tekstem), recaptcha_v2, recaptcha_v3, hcaptcha. Zwraca token/string do wklejenia w formularz.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": ["image", "recaptcha_v2", "recaptcha_v3", "hcaptcha"],
                            "description": "Typ captchy"
                        },
                        "api_key": {
                            "type": "string",
                            "description": "Klucz API serwisu (2captcha.com, anti-captcha.com lub capmonster.cloud)"
                        },
                        "service": {
                            "type": "string",
                            "enum": ["2captcha", "anti-captcha", "capmonster"],
                            "description": "Serwis do rozwiązania (domyślnie: 2captcha)"
                        },
                        "site_key": {
                            "type": "string",
                            "description": "data-sitekey z HTML strony (dla recaptcha_v2/v3/hcaptcha)"
                        },
                        "page_url": {
                            "type": "string",
                            "description": "URL strony z captchą (dla recaptcha/hcaptcha)"
                        },
                        "image_base64": {
                            "type": "string",
                            "description": "PNG/JPG zakodowany base64 (dla type=image). Możesz użyć screenshot() żeby go dostać."
                        },
                        "action": {
                            "type": "string",
                            "description": "Akcja reCAPTCHA v3 np. 'login', 'register' (tylko dla recaptcha_v3)"
                        },
                        "min_score": {
                            "type": "number",
                            "description": "Minimalny score reCAPTCHA v3 (0.3-0.9, domyślnie 0.3)"
                        }
                    },
                    "required": ["type", "api_key"]
                }
            },
            {
                "name": "go_back",
                "description": "Cofa do poprzedniej strony (historia nawigacji). Zwraca WAP-XML poprzedniej strony. Działa tylko jeśli wcześniej użyto navigate().",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "scroll_down",
                "description": "Przewija stronę w dół o N pikseli (domyślnie 800) i zwraca odświeżony WAP-XML. Przydatne do infinite scroll, lazy-load i stron paginowanych przez scroll.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pixels": {
                            "type": "integer",
                            "description": "Liczba pikseli do przewinięcia (domyślnie 800)"
                        }
                    }
                }
            },
            {
                "name": "execute_js",
                "description": "Wykonuje dowolny JavaScript na aktualnej stronie i zwraca wynik. Escape hatch gdy WAP-XML nie wystarczy - np. ekstrakcja danych ze złożonych SPA, sprawdzenie stanu aplikacji, odczyt localStorage.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "code": {
                            "type": "string",
                            "description": "Kod JS do wykonania na stronie. Powinien zwracać wartość (string/number/object). Np: \"document.querySelectorAll('.price').length\""
                        }
                    },
                    "required": ["code"]
                }
            },
            {
                "name": "extract_table",
                "description": "Wyciąga tabele HTML ze strony jako JSON. Zwraca array tabel z nagłówkami i wierszami. Idealne do kursów walut, cenników, danych statystycznych, tabel wyników.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "selector": {
                            "type": "string",
                            "description": "CSS selector tabel (domyślnie 'table'). Np. 'table.data-table' lub '.results table'"
                        }
                    }
                }
            }
        ]
    })
}

// ---------------------------------------------------------------------------
// Główna pętla MCP
// ---------------------------------------------------------------------------

pub struct McpServer {
    /// Lazy-initialized - Chrome starts only on first tool call, not at startup.
    /// initialize/tools/list respond immediately without waiting for Chrome.
    browser: Arc<Mutex<Option<AgentBrowser>>>,
}

impl McpServer {
    pub fn new() -> Self {
        Self {
            browser: Arc::new(Mutex::new(None)),
        }
    }

    /// Startuje pętlę stdin/stdout - blokuje do EOF lub błędu
    pub async fn run(self) -> Result<()> {
        info!("MCP server starting on stdio...");

        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let mut reader = BufReader::new(stdin).lines();

        while let Some(line) = reader.next_line().await? {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            debug!("← {}", &line[..line.len().min(200)]);

            // Sprawdź czy to notification (brak id) - nie wysyłaj odpowiedzi
            let is_notification = {
                serde_json::from_str::<serde_json::Value>(&line)
                    .map(|v| v.get("id").is_none() || v.get("id") == Some(&serde_json::Value::Null))
                    .unwrap_or(false)
            };

            if is_notification {
                // Tylko procesuj, nie odpowiadaj (MCP spec: notifications mają odpowiedzi)
                let _ = self.handle_line(&line).await;
                continue;
            }

            let response = self.handle_line(&line).await;
            let response_str = serde_json::to_string(&response)? + "\n";
            debug!("→ {}", &response_str[..response_str.len().min(200)]);

            stdout.write_all(response_str.as_bytes()).await?;
            stdout.flush().await?;
        }

        info!("MCP server shutting down (stdin closed)");
        Ok(())
    }

    async fn handle_line(&self, line: &str) -> Response {
        // Parse request
        let req: Request = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                error!("JSON parse error: {}", e);
                return Response::err(Value::Null, -32700, format!("Parse error: {}", e));
            }
        };

        if req.jsonrpc != "2.0" {
            return Response::err(req.id.unwrap_or(Value::Null), -32600, "Invalid Request");
        }

        let id = req.id.clone().unwrap_or(Value::Null);

        match req.method.as_str() {
            // --- Handshake MCP ---
            "initialize" => {
                info!("MCP initialize");
                Response::ok(id, json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "rust-browser-mcp",
                        "version": "0.1.0"
                    }
                }))
            }

            "notifications/initialized" | "initialized" => {
                // Notification - odpowiadamy null result
                Response::ok(id, Value::Null)
            }

            "tools/list" => {
                Response::ok(id, tool_definitions())
            }

            "tools/call" => {
                self.handle_tool_call(id, &req.params).await
            }

            "ping" => Response::ok(id, json!({})),

            unknown => {
                warn!("Unknown method: {}", unknown);
                Response::err(id, -32601, format!("Method not found: {}", unknown))
            }
        }
    }

    async fn handle_tool_call(&self, id: Value, params: &Value) -> Response {
        let tool_name = match params.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => return Response::err(id, -32602, "Missing tool name"),
        };

        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        info!("Tool call: {} args={}", tool_name, args);

        let result = self.dispatch_tool(&tool_name, &args).await;

        match result {
            Ok(content) => Response::ok(id, json!({ "content": content })),
            Err(e) => {
                error!("Tool {} error: {}", tool_name, e);
                Response::err(id, -32000, format!("Tool error: {}", e))
            }
        }
    }

    async fn dispatch_tool(&self, name: &str, args: &Value) -> Result<Value> {
        // Lazy-init + dispatch w jednym locku - unikamy deadlocka podwójnego lock().await
        let mut browser_guard = self.browser.lock().await;
        if browser_guard.is_none() {
            info!("Lazy-starting Chrome on first tool call...");
            let b = AgentBrowser::new().await?;
            *browser_guard = Some(b);
            info!("Chrome ready.");
        }
        let browser = browser_guard.as_mut()
            .ok_or_else(|| anyhow::anyhow!("Browser not initialized"))?;

        match name {
            "navigate" => {
                let url = args["url"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing 'url' argument"))?
                    .to_string();
                let b = &mut *browser;
                let xml = b.chrome.navigate_wap(&url, &mut b.state).await?;
                Ok(json!([{"type": "text", "text": xml}]))
            }

            // Tylko czysty tekst ze strony (jak RSS item body)
            "get_text" => {
                if browser.state.url.is_empty() {
                    return Ok(json!([{"type": "text", "text": "Brak załadowanej strony. Użyj navigate(url) najpierw."}]));
                }
                // Wstrzykujemy uproszczony extractor - tylko tekst, zero tagów
                const TEXT_JS: &str = r#"
(function() {
    function isNoise(el) {
        const tag = el.tagName && el.tagName.toLowerCase();
        if (['script','style','noscript','iframe','nav','header','footer','aside'].includes(tag)) return true;
        const cls = (el.className && typeof el.className === 'string' ? el.className : '').toLowerCase();
        const nid = (el.id || '').toLowerCase();
        const noiseWords = ['nav','menu','sidebar','footer','header','cookie','popup',
                            'modal','banner','advert','promo','social','share','comment'];
        return noiseWords.some(w => cls.includes(w) || nid.includes(w));
    }
    function findMain() {
        return document.querySelector('article') || document.querySelector('main') ||
               document.querySelector('[role="main"]') || document.body;
    }
    const main = findMain();
    const lines = [`# ${document.title}`, `URL: ${window.location.href}`, ''];
    const seen = new Set();
    const walker = document.createTreeWalker(main, NodeFilter.SHOW_ELEMENT,
        { acceptNode: n => isNoise(n) ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT });
    let node = walker.nextNode();
    while (node) {
        const tag = node.tagName.toLowerCase();
        if (['h1','h2','h3'].includes(tag)) {
            const t = (node.innerText || '').replace(/\s+/g,' ').trim();
            if (t && !seen.has(t)) { seen.add(t); lines.push(`\n## ${t}`); }
        }
        if (tag === 'p') {
            const t = (node.innerText || '').replace(/\s+/g,' ').trim();
            if (t && t.length >= 20 && !seen.has(t)) { seen.add(t); lines.push(t); }
        }
        node = walker.nextNode();
    }
    return lines.join('\n');
})()
"#;
                // Otwieramy stronę i wstrzykujemy text extractor
                let url = browser.state.url.clone();
                let text = browser
                    .chrome
                    .evaluate_on_url(&url, TEXT_JS)
                    .await?;
                Ok(json!([{"type": "text", "text": text}]))
            }

            // Tylko lista linków (id, href, label)
            "get_links" => {
                if browser.state.url.is_empty() {
                    return Ok(json!([{"type": "text", "text": "<links />"}]));
                }
                const LINKS_JS: &str = r#"
(function() {
    let id = 1;
    const seen = new Set();
    const lines = ['<links>'];
    function esc(s) { return String(s||'').replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;'); }
    function text(el) { return (el.innerText||el.textContent||'').replace(/\s+/g,' ').trim(); }
    function abs(h) { try { return new URL(h, window.location.href).href; } catch(e) { return h; } }
    document.querySelectorAll('a[href]').forEach(a => {
        const href = a.getAttribute('href') || '';
        if (!href || href.startsWith('#') || href.startsWith('javascript:')) return;
        const resolved = abs(href);
        if (seen.has(resolved)) return;
        seen.add(resolved);
        const label = esc(text(a) || a.getAttribute('aria-label') || href);
        if (!label) return;
        lines.push(`  <link id="${id++}" href="${esc(resolved)}">${label}</link>`);
    });
    lines.push('</links>');
    return lines.join('\n');
})()
"#;
                let url = browser.state.url.clone();
                let result = browser.chrome.evaluate_on_url(&url, LINKS_JS).await?;
                Ok(json!([{"type": "text", "text": result}]))
            }

            "get_content" => {
                if browser.state.url.is_empty() {
                    return Ok(json!([{"type": "text", "text": "<error>Brak załadowanej strony. Użyj navigate(url) najpierw.</error>"}]));
                }
                let url = browser.state.url.clone();
                let b = &mut *browser;
                let xml = b.chrome.navigate_wap(&url, &mut b.state).await?;
                Ok(json!([{"type": "text", "text": xml}]))
            }

            "click_link" => {
                let id = args["id"]
                    .as_u64()
                    .ok_or_else(|| anyhow::anyhow!("Missing 'id' argument"))? as u32;

                // Pobierz URL z WAP-XML przez JS - szukamy linku o danym id
                // (state nie trzyma elementów od strony Chrome - ID są w JS)
                let find_js = format!(
                    r#"(function(){{
                        let n = 1;
                        const seen = new Set();
                        for (const a of document.querySelectorAll('a[href]')) {{
                            const href = a.getAttribute('href') || '';
                            if (!href || href.startsWith('#') || href.startsWith('javascript:')) continue;
                            const resolved = new URL(href, window.location.href).href;
                            if (seen.has(resolved)) continue;
                            seen.add(resolved);
                            if (n === {target_id}) return resolved;
                            n++;
                        }}
                        return null;
                    }})()"#,
                    target_id = id
                );
                let url = browser.state.url.clone();
                let target: Option<String> = browser
                    .chrome
                    .evaluate_on_url_opt(&url, &find_js)
                    .await?;

                let target_url = target
                    .ok_or_else(|| anyhow::anyhow!("Link o id={} nie znaleziony na stronie", id))?;

                let b = &mut *browser;
                let xml = b.chrome.navigate_wap(&target_url, &mut b.state).await?;
                Ok(json!([{"type": "text", "text": xml}]))
            }

            "click_button" => {
                let id = args["id"]
                    .as_u64()
                    .ok_or_else(|| anyhow::anyhow!("Missing 'id' argument"))? as u32;

                let b = &mut *browser;
                let xml = b.chrome.click_wap_element(id, &mut b.state).await?;
                Ok(json!([{"type": "text", "text": xml}]))
            }

            "fill_and_submit" => {
                let url = args["url"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing 'url' argument"))?
                    .to_string();

                let fields_obj = args["fields"]
                    .as_object()
                    .ok_or_else(|| anyhow::anyhow!("'fields' must be an object {{name: value}}"))?;

                let fields: Vec<(String, String)> = fields_obj
                    .iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                    .collect();

                let form_selector = args["form_selector"]
                    .as_str()
                    .unwrap_or("form")
                    .to_string();

                let b = &mut *browser;
                let xml = b.chrome
                    .fill_and_submit(&url, &fields, &form_selector, &mut b.state)
                    .await?;
                Ok(json!([{"type": "text", "text": xml}]))
            }

            "screenshot" => {
                let url = args["url"]
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| browser.state.url.clone());

                if url.is_empty() {
                    return Err(anyhow::anyhow!("Brak URL - użyj navigate() najpierw lub podaj url"));
                }

                let b64 = browser.chrome.screenshot(&url).await?;
                info!("Screenshot: {} b64 chars → MCP image content", b64.len());
                // MCP type:image - Claude/GPT-4o widzi obrazek bezpośrednio (vision)
                Ok(json!([
                    {"type": "image", "data": b64, "mimeType": "image/png"},
                    {"type": "text",  "text": format!("Screenshot URL: {}", url)}
                ]))
            }

            "get_state" => Ok(json!([{"type": "text", "text": json!({
                "url": browser.state.url,
                "title": browser.state.title,
            }).to_string()}])),

            "solve_captcha" => {
                let api_key = args["api_key"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing 'api_key'"))?
                    .to_string();

                let service = match args["service"].as_str().unwrap_or("2captcha") {
                    "anti-captcha" => CaptchaService::AntiCaptcha,
                    "capmonster"   => CaptchaService::CapMonster,
                    _              => CaptchaService::TwoCaptcha,
                };

                let config = CaptchaConfig {
                    api_key,
                    service,
                    ..CaptchaConfig::default()
                };

                let solver = CaptchaSolver::new(config)?;

                let captcha_type = args["type"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing 'type'"))?;

                let task = match captcha_type {
                    "image" => {
                        let img = args["image_base64"]
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("image_base64 required for type=image"))?
                            .to_string();
                        CaptchaTask::Image { base64_image: img }
                    }
                    "recaptcha_v2" => {
                        let site_key = args["site_key"]
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("site_key required for recaptcha_v2"))?
                            .to_string();
                        let page_url = args["page_url"]
                            .as_str()
                            .unwrap_or(&browser.state.url)
                            .to_string();
                        CaptchaTask::RecaptchaV2 { site_key, page_url }
                    }
                    "recaptcha_v3" => {
                        let site_key = args["site_key"]
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("site_key required for recaptcha_v3"))?
                            .to_string();
                        let page_url = args["page_url"]
                            .as_str()
                            .unwrap_or(&browser.state.url)
                            .to_string();
                        let action = args["action"]
                            .as_str()
                            .unwrap_or("submit")
                            .to_string();
                        let min_score = args["min_score"]
                            .as_f64()
                            .unwrap_or(0.3) as f32;
                        CaptchaTask::RecaptchaV3 { site_key, page_url, action, min_score }
                    }
                    "hcaptcha" => {
                        let site_key = args["site_key"]
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("site_key required for hcaptcha"))?
                            .to_string();
                        let page_url = args["page_url"]
                            .as_str()
                            .unwrap_or(&browser.state.url)
                            .to_string();
                        CaptchaTask::HCaptcha { site_key, page_url }
                    }
                    unknown => anyhow::bail!("Unknown captcha type: {}", unknown),
                };

                // Solve - może trwać do 2 minut (czekamy na człowieka)
                let solution = solver.solve(task).await?;

                Ok(json!([{"type": "text", "text": json!({
                    "solution": solution.solution,
                    "type": solution.captcha_type,
                    "task_id": solution.task_id,
                    "hint": "Wstaw solution do fill_and_submit() jako wartość pola g-recaptcha-response lub h-captcha-response"
                }).to_string()}]))
            }

            "go_back" => {
                let prev = browser.state.pop_history();
                match prev {
                    Some(url) => {
                        let b = &mut *browser;
                        let xml = b.chrome.navigate_wap(&url, &mut b.state).await?;
                        Ok(json!([{"type": "text", "text": xml}]))
                    }
                    None => Ok(json!([{"type": "text", "text": "<error>Brak historii nawigacji. Użyj navigate() najpierw.</error>"}]))
                }
            }

            "scroll_down" => {
                if browser.state.url.is_empty() {
                    return Ok(json!([{"type": "text", "text": "<error>Brak załadowanej strony. Użyj navigate(url) najpierw.</error>"}]));
                }
                let pixels = args["pixels"].as_u64().unwrap_or(800) as u32;
                let url = browser.state.url.clone();
                let b = &mut *browser;
                let xml = b.chrome.scroll_and_extract(&url, pixels, &mut b.state).await?;
                Ok(json!([{"type": "text", "text": xml}]))
            }

            "execute_js" => {
                if browser.state.url.is_empty() {
                    return Ok(json!([{"type": "text", "text": "<error>Brak załadowanej strony. Użyj navigate(url) najpierw.</error>"}]));
                }
                let code = args["code"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing 'code' argument"))?;
                let url = browser.state.url.clone();
                let result = browser.chrome.execute_js_raw(&url, code).await?;
                Ok(json!([{"type": "text", "text": result}]))
            }

            "extract_table" => {
                if browser.state.url.is_empty() {
                    return Ok(json!([{"type": "text", "text": "[]"}]));
                }
                let selector = args["selector"].as_str();
                let url = browser.state.url.clone();
                let result = browser.chrome.extract_tables(&url, selector).await?;
                Ok(json!([{"type": "text", "text": result}]))
            }

            unknown => Err(anyhow::anyhow!("Unknown tool: {}", unknown)),
        }
    }
}
