/// rust-browser-mcp
///
/// Przeglądarka jako MCP server dla agentów AI.
/// Chrome headless (CDP) + JS DOM extractor = WAP-style semantic XML.
///
/// Użycie:
///   rust-browser-mcp
///
/// Wymaga zainstalowanego Google Chrome lub Chromium.
///
/// Podłączenie do Claude Desktop (~/.config/claude/claude_desktop_config.json):
///   {
///     "mcpServers": {
///       "browser": {
///         "command": "C:\\path\\to\\rust-browser-mcp.exe"
///       }
///     }
///   }
///
/// Podłączenie do VS Code Copilot (.vscode/mcp.json):
///   {
///     "servers": {
///       "rust-browser": {
///         "type": "stdio",
///         "command": "rust-browser-mcp"
///       }
///     }
///   }
///
/// Dostępne tools:
///   navigate(url)                          → WAP-XML: treść + formularze + linki
///   get_text()                             → czysty tekst (jak RSS item)
///   get_links()                            → lista linków z ID
///   get_content()                          → WAP-XML z aktualnej strony
///   click_link(id)                         → nawigacja do linku o ID
///   fill_and_submit(url, fields, selector) → wypełnij i wyślij formularz
///   screenshot(url?)                       → PNG base64 dla Vision LLM
///   get_state()                            → bieżący URL i tytuł

mod browser;
mod captcha;
mod mcp;
mod types;

use anyhow::Result;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    // Logowanie TYLKO do stderr - nie zaburza stdio MCP transport
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "rust_browser_mcp=info".into()),
        )
        .init();

    info!("rust-browser-mcp starting...");
    info!("Chrome will start on first tool call (lazy init).");

    let server = mcp::McpServer::new();
    server.run().await?;

    Ok(())
}
