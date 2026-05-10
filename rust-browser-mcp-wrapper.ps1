# Wrapper dla rust-browser-mcp - zabija stare instancje Chrome przed startem
# VS Code odpala ten skrypt jako MCP server zamiast binary

# Zabij orphan Chrome z poprzednich sesji
Get-Process chrome -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

# Wyczysc profile lock
$profile = "$env:TEMP\rust_browser_mcp_profile"
Remove-Item $profile -Recurse -Force -ErrorAction SilentlyContinue

# Start MCP server - przekaz stdin/stdout bez modyfikacji
& "C:\rust-browser\target\release\rust-browser-mcp.exe" @args
