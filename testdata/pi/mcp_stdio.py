#!/usr/bin/env python3
"""Deterministic, offline MCP fixture shared by stdio, HTTP and real-Pi tests."""
import json
import os
import sys
import time

TOOLS = [
    {"name": "read_file", "description": "Read a fixture", "inputSchema": {"type": "object", "properties": {}}, "annotations": {"readOnlyHint": True}},
    {"name": "write-note", "description": "Write a fixture note", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}, "annotations": {"destructiveHint": True}},
    {"name": "edit_file", "description": "Edit by name heuristic", "inputSchema": {"type": "object", "properties": {}}},
]

def respond(message):
    method = message.get("method")
    if "id" not in message:
        return None
    if method == "initialize":
        if os.environ.get("RRSS_FIXTURE_OLD_PROTOCOL") and message["params"]["protocolVersion"] != "2025-06-18":
            return {"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32602, "message": "Unsupported protocol version"}}
        result = {"protocolVersion": message["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "rrss-fixture", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": TOOLS}
    elif method == "tools/call":
        name = message["params"]["name"]
        marker = os.environ.get("RRSS_FIXTURE_CALLS")
        if marker:
            with open(marker, "a", encoding="utf-8") as output:
                output.write(name + "\n")
        result = {"content": [{"type": "text", "text": "fixture tool completed"}]}
    elif method == "ping":
        result = {}
    else:
        return {"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "Unknown method"}}
    return {"jsonrpc": "2.0", "id": message["id"], "result": result}

if __name__ == "__main__":
    marker = os.environ.get("RRSS_FIXTURE_STARTED")
    if marker:
        from pathlib import Path
        Path(marker).touch()
    if "--fail" in sys.argv:
        print("Missing fixture input " + os.environ.get("TEST_SECRET", ""), file=sys.stderr, flush=True)
        sys.exit(3)
    if "--hang" in sys.argv:
        time.sleep(120)
    for line in sys.stdin:
        answer = respond(json.loads(line))
        if answer is not None:
            print(json.dumps(answer), flush=True)
