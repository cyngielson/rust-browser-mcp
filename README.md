# rust-browser-mcp

**Give eyes to your AI agent.**

Chrome as an MCP tool. Your agent navigates real pages, reads content, fills forms, takes screenshots and executes JavaScript — all through a compact semantic XML format instead of raw HTML dumps.

Zero Node.js. Zero Python. Native Rust binary, ~12MB, starts in under 2 seconds.

---

## What it does

Your agent calls `navigate("https://example.com")` and gets back this:

```xml
<page url="https://news.ycombinator.com/" title="Hacker News">
  <h3>Show HN: I built X because Y</h3>
  <link id="4" href="https://news.ycombinator.com/item?id=123">comments</link>
  <link id="5" href="https://news.ycombinator.com/newest">new</link>
</page>
```

~2KB instead of 500KB. No noise, no scripts, no ads. Just content and actionable elements with numeric IDs.

---

## Why not Playwright MCP?

|  | **rust-browser-mcp** | Playwright MCP |
|---|---|---|
| Runtime | Native Rust binary | Node.js required |
| Output format | Compact WAP-XML (~2KB) | Raw DOM / snapshots |
| Memory | Chrome only | Chrome + Node process |
| Bot detection | STEALTH_JS patches | Detectable headless |
| Binary size | 12MB | 200MB+ with deps |

Both use real Chrome CDP. This one just does not need a Node.js runtime in between.

---

## Requirements

- Windows 10/11 (or Linux)
- Google Chrome 115+
- Rust 2021 (to build)

---

## Installation

```powershell
git clone https://github.com/yourname/rust-browser-mcp
cd rust-browser-mcp
cargo build --release
# binary: target\release\rust-browser-mcp.exe
```

Override Chrome path: `CHROME_EXE=C:\path\to\chrome.exe`

---

## VS Code (mcp.json)

```json
{
  "servers": {
    "rust-browser": {
      "command": "C:\\rust-browser\\target\\release\\rust-browser-mcp.exe",
      "cwd": "C:\\rust-browser",
      "env": { "RUST_LOG": "error" },
      "type": "stdio"
    }
  }
}
```

## Claude Desktop

```json
{
  "mcpServers": {
    "browser": {
      "command": "C:\\rust-browser\\target\\release\\rust-browser-mcp.exe"
    }
  }
}
```

Chrome launches lazily on first tool call. `initialize` and `tools/list` respond instantly.

---

## Tools (14)

### Navigation
| Tool | Description |
|------|-------------|
| `navigate(url)` | Open URL, returns WAP-XML (content + forms + links with IDs) |
| `go_back()` | Previous page (history stack, max 20) |
| `get_state()` | Current URL and title |

### Content
| Tool | Description |
|------|-------------|
| `get_text()` | Plain text only — headings + paragraphs, no tags. Like RSS article body |
| `get_links()` | Link list — id, href, label |
| `get_content()` | Full WAP-XML of current page, no new request |
| `extract_table(selector?)` | HTML tables as JSON array with headers and rows |

### Interaction
| Tool | Description |
|------|-------------|
| `click_link(id)` | Navigate to link by WAP id |
| `click_button(id)` | Click button by WAP id, waits for navigation/re-render |
| `fill_and_submit(url, fields, form_selector?)` | Fill + submit form, returns result WAP-XML |
| `scroll_down(pixels?)` | Scroll down N px (default 800), waits for lazy-load, returns fresh WAP-XML |
| `execute_js(code)` | Run arbitrary JS on current page, returns result |

### Vision & Captcha
| Tool | Description |
|------|-------------|
| `screenshot(url?)` | PNG as base64 for visual verification with vision LLMs |
| `solve_captcha(type, api_key, ...)` | Solve via 2captcha / anti-captcha / CapMonster |

---

## WAP-XML format

```xml
<page url="https://example.com/search" title="Search Results">
  <h1>Results for: rust async</h1>
  <p>Found 42 results.</p>
  <form id="1" method="GET" action="/search">
    <input id="2" name="q" type="search" placeholder="Search..." />
    <button id="3">Search</button>
  </form>
  <link id="4" href="/result/1">First result title</link>
  <link id="5" href="/result/2">Second result title</link>
</page>
```

All interactive elements have numeric IDs. Pass directly to `click_button(3)` or `click_link(4)`.

---

## Agent workflow example

```
navigate("https://hn.algolia.com/")
  -> WAP-XML with search form

fill_and_submit(url, {"query": "rust async"}, "form")
  -> WAP-XML with results

click_link(7)
  -> WAP-XML of article page

get_text()
  -> clean article text, no noise

screenshot()
  -> base64 PNG for vision verification
```

---

## Stealth

Chrome launches with anti-bot flags plus STEALTH_JS injected on every page:
- `navigator.webdriver` removed
- `navigator.plugins` / `navigator.languages` / `window.chrome` patched to look like real Chrome
- CDP artifacts (`cdc_*`, `__webdriver_*`) removed
- Screen: 1920x1080, color depth 24

---

## Architecture

```
Claude / VS Code Copilot
         | JSON-RPC 2.0 (stdio)
    McpServer (Rust)
         | lazy init on first call
    AgentBrowser
         | CDP WebSocket
    Chrome headless (manual spawn)
         | JS injection
    Live DOM -> WAP-XML (~2KB)
```

Manual Chrome spawn instead of `Browser::launch()` — the chromiumoxide launch API breaks on Chrome 145+/Windows. Fix: find free port, spawn with `--remote-debugging-port=PORT`, poll `/json/version` via raw TCP, connect with `Browser::connect(ws_url)`.

---

## License

MIT