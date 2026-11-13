#!/usr/bin/env python3
"""Drives `visioncraft-cli mcp` over stdio: builds a garden step by step, takes a picture,
sets the time to dusk, takes another, saves. Prints every request and reply line (image data
shortened) and writes the pictures next to the project.

usage: python3 -I mcp_demo.py /path/to/visioncraft-cli OUT_DIR
"""
import base64, json, subprocess, sys

cli, out = sys.argv[1], sys.argv[2]
p = subprocess.Popen([cli, "mcp", "--automation-read-root", out, "--automation-write-root", out],
                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
n = 0

def short(line):
    v = json.loads(line)
    for block in v.get("result", {}).get("content", []):
        if block.get("type") == "image":
            block["data"] = block["data"][:24] + f"...({len(block['data'])} base64 chars)"
    return json.dumps(v)

def send(method, params=None, notify=False):
    global n
    msg = {"jsonrpc": "2.0", "method": method}
    if params is not None:
        msg["params"] = params
    if not notify:
        n += 1
        msg["id"] = n
    line = json.dumps(msg)
    print(">>", line, flush=True)
    p.stdin.write(line + "\n"); p.stdin.flush()
    if notify:
        return None
    reply = p.stdout.readline()
    print("<<", short(reply), flush=True)
    return json.loads(reply)

def tool(name, args):
    return send("tools/call", {"name": name, "arguments": args})

def picture(reply, name):
    for block in reply["result"]["content"]:
        if block["type"] == "image":
            with open(f"{out}/{name}", "wb") as f:
                f.write(base64.b64decode(block["data"]))

send("initialize", {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "mcp_demo", "version": "1"}})
send("notifications/initialized", notify=True)
tool("command_list", {"brief": True, "filter": "shape."})
tool("command_run", {"id": "site.set", "params": {"latitude": 51.05, "longitude": 3.72, "utc_offset_hours": 2}})
tool("command_run", {"id": "time.set", "params": {"year": 2026, "month": 6, "day": 21, "hour": 15}})
tool("command_run", {"id": "shape.add_area", "params": {"name": "Lawn", "outline": [[-10, -2], [10, -2], [10, 10], [-10, 10]], "material": "lawn"}})
tool("command_run", {"id": "shape.add_extrusion", "params": {"name": "House", "outline": [[-5, -10], [5, -10], [5, -3], [-5, -3]], "height": 6, "material": "brick"}})
tool("command_run", {"id": "species.list", "params": {"category": "hedge", "filter": "carpinus"}})
tool("command_run", {"id": "planting.add_spline", "params": {"name": "Hedge", "asset_palette": [{"asset": "flora:carpinus-hedge", "weight": 1}], "points": [[-10, 10], [10, 10]], "spacing": {"every": 0.8}}})
tool("command_batch", {"steps": [
    {"id": "light.add", "params": {"name": "Garden lamp", "position": [3, 2.2, -1], "lumens": 1200}},
    {"id": "viewpoint.add", "params": {"name": "From the gate", "position": [12, 2.2, 16], "target": [0, 2, -4]}}]})
tool("doc_inspect", {"faces": False})
picture(tool("doc_render_preview", {"viewpoint": "From the gate", "max_side": 960}), "afternoon.png")
tool("command_run", {"id": "query.sun", "params": {}})
tool("command_run", {"id": "time.set", "params": {"hour": 21.75}})
picture(tool("doc_render_preview", {"viewpoint": "From the gate", "max_side": 960}), "dusk.png")
tool("command_run", {"id": "file.save", "params": {"path": "garden.visioncraft"}})
p.stdin.close()
p.wait()
