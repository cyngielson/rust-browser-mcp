import subprocess, json, sys, base64, os

exe = r"C:\rust-browser\target\release\rust-browser-mcp.exe"

msgs = [
    {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}},
    {"jsonrpc":"2.0","method":"notifications/initialized","params":{}},
    {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"navigate","arguments":{"url":"https://en.wikipedia.org/wiki/Rust_(programming_language)"}}},
]

input_str = "\n".join(json.dumps(m) for m in msgs) + "\n"

proc = subprocess.run(
    [exe],
    input=input_str.encode("utf-8"),
    capture_output=True,
    timeout=60
)

stdout = proc.stdout.decode("utf-8", errors="replace")
stderr = proc.stderr.decode("utf-8", errors="replace")

for line in stdout.splitlines():
    try:
        obj = json.loads(line)
        rid = obj.get("id")
        if rid == 2 and "result" in obj:
            text = obj["result"]["content"][0]["text"]
            print(f"Wikipedia WAP length: {len(text)} chars")
            print(text[:2000])
            print("...")
            if "<truncated" in text:
                print("=== TRUNCATION FOUND ===")
            else:
                print("=== NO TRUNCATION ===")
    except:
        pass

print("STDERR last 5:")
for l in stderr.splitlines()[-5:]:
    print(l)
