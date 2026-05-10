import subprocess, json, sys, time

exe = r"C:\rust-browser\target\release\rust-browser-mcp.exe"

msgs = [
    {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}},
    {"jsonrpc":"2.0","method":"notifications/initialized","params":{}},
    {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"navigate","arguments":{"url":"https://example.com"}}},
    {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_text","arguments":{}}},
    {"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_links","arguments":{}}},
    {"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_state","arguments":{}}},
    {"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"click_link","arguments":{"id":1}}},
    {"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"get_state","arguments":{}}},
]

input_str = "\n".join(json.dumps(m) for m in msgs) + "\n"

proc = subprocess.run(
    [exe],
    input=input_str.encode("utf-8"),
    capture_output=True,
    timeout=60
)

print("=== STDOUT ===")
for line in proc.stdout.decode("utf-8", errors="replace").splitlines():
    try:
        obj = json.loads(line)
        rid = obj.get("id")
        if "result" in obj:
            content = obj["result"]
            if isinstance(content, dict) and "content" in content:
                text = content["content"][0]["text"] if content["content"] else ""
                print(f"[id={rid}] {text[:300]}")
            else:
                print(f"[id={rid}] {json.dumps(content)[:200]}")
        elif "error" in obj:
            print(f"[id={rid}] ERROR: {obj['error']}")
    except:
        print(f"RAW: {line[:200]}")

print("\n=== STDERR (last 15 lines) ===")
for line in proc.stderr.decode("utf-8", errors="replace").splitlines()[-15:]:
    print(line)
