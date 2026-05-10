import subprocess, json, sys, base64, os

exe = r"C:\rust-browser\target\release\rust-browser-mcp.exe"

msgs = [
    {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}},
    {"jsonrpc":"2.0","method":"notifications/initialized","params":{}},
    # Real article page
    {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"navigate","arguments":{"url":"https://en.wikipedia.org/wiki/Rust_(programming_language)"}}},
    # Screenshot for vision verification
    {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"screenshot","arguments":{}}},
    # Google search form test
    {"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"navigate","arguments":{"url":"https://duckduckgo.com"}}},
    {"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"fill_and_submit","arguments":{"url":"https://duckduckgo.com","fields":{"q":"rust programming language MCP"},"form_selector":"form"}}},
]

input_str = "\n".join(json.dumps(m) for m in msgs) + "\n"

proc = subprocess.run(
    [exe],
    input=input_str.encode("utf-8"),
    capture_output=True,
    timeout=90
)

print("=== RESULTS ===")
for line in proc.stdout.decode("utf-8", errors="replace").splitlines():
    try:
        obj = json.loads(line)
        rid = obj.get("id")
        if "result" in obj:
            content = obj["result"]
            if isinstance(content, dict) and "content" in content:
                text = content["content"][0]["text"] if content["content"] else ""
                if rid == 3:  # screenshot
                    # Save PNG
                    if text.startswith("data:image"):
                        b64 = text.split(",",1)[1]
                    else:
                        b64 = text
                    try:
                        png = base64.b64decode(b64)
                        path = r"C:\rust-browser\test_wiki_screenshot.png"
                        open(path, "wb").write(png)
                        print(f"[id={rid}] Screenshot saved: {len(png)//1024} KB → {path}")
                    except Exception as e:
                        print(f"[id={rid}] Screenshot base64 len={len(text)}, decode error: {e}")
                else:
                    print(f"\n[id={rid}] {text[:600]}")
                    print(f"         ... ({len(text)} chars total)")
            else:
                print(f"[id={rid}] {json.dumps(content)[:300]}")
        elif "error" in obj:
            print(f"[id={rid}] ERROR: {obj['error']}")
    except:
        pass

print("\n=== STDERR ===")
for line in proc.stderr.decode("utf-8", errors="replace").splitlines()[-20:]:
    print(line)
