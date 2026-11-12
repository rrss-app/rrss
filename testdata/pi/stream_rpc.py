#!/usr/bin/env python3
"""Fake Pi RPC that emits a fixed number of text deltas. No model and no network."""
import json
import os
import sys
import time
import uuid
from pathlib import Path

# The host emits chat:delta only when 30ms have passed since the previous one.
# Sleeping the scene interval alone can coalesce two deltas into one snapshot.
PACE_FLOOR_MS = 45


def emit(event):
    print(json.dumps(event, ensure_ascii=False), flush=True)


def required_int(name):
    raw = os.environ.get(name, "").strip()
    if not raw.isdigit() or int(raw) < 1:
        raise SystemExit(f"{name} must be a positive integer")
    return int(raw)


if "--version" in sys.argv:
    print("1.5.0")
    sys.exit(0)


def current_model():
    """Report the model RRSS asked for, like a Pi that resolved `--model provider/id`."""
    ref = sys.argv[sys.argv.index("--model") + 1] if "--model" in sys.argv else ""
    provider, _, model_id = ref.partition("/")
    return {"provider": provider, "id": model_id} if model_id else None


snapshots = required_int("RRSS_STREAM_SNAPSHOTS")
interval_ms = required_int("RRSS_STREAM_INTERVAL_MS")
gap = max(interval_ms, PACE_FLOOR_MS) / 1000
session_file = None

for line in sys.stdin:
    command = json.loads(line)
    kind = command.get("type")
    response = {"type": "response", "id": command.get("id"), "command": kind, "success": True, "data": {}}
    try:
        if kind == "new_session":
            session_dir = Path(sys.argv[sys.argv.index("--session-dir") + 1])
            session_dir.mkdir(parents=True, exist_ok=True)
            session_file = session_dir / f"fake-{uuid.uuid4()}.jsonl"
            session_file.write_text("1", encoding="utf-8")
            response["data"] = {"cancelled": False}
        elif kind == "get_state":
            # The startup handshake asks before any session exists.
            response["data"] = {"model": current_model(), "sessionFile": str(session_file) if session_file else None,
                                "isStreaming": False}
        elif kind == "get_available_thinking_levels":
            response["data"] = {"levels": ["off"]}
        elif kind == "prompt":
            emit(response)
            emit({"type": "agent_start"})
            emit({"type": "message_end", "message": {"role": "user", "content": command.get("message", "")}})
            time.sleep(0.3)
            piece = "片"
            full = ""
            for _ in range(snapshots):
                full += piece
                emit({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "delta": piece}})
                time.sleep(gap)
            emit({"type": "message_end", "message": {"role": "assistant", "content": full}})
            emit({"type": "agent_settled"})
            continue
        elif kind == "abort":
            emit({"type": "agent_settled"})
        elif kind == "switch_session":
            session_file = Path(command["sessionPath"])
            if not session_file.is_file():
                raise RuntimeError("saved session missing")
            response["data"] = {"cancelled": False}
        else:
            response["data"] = {"cancelled": False}
        emit(response)
    except Exception as error:
        response.update(success=False, error=str(error))
        emit(response)
