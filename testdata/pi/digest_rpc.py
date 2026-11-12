#!/usr/bin/env python3
"""Deterministic Pi RPC fixture. Only reads RRSS's isolated test DB; never calls a model."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import uuid

if "--version" in sys.argv:
    print("1.5.0")
    sys.exit(0)


def current_model():
    """Report the model RRSS asked for, like a Pi that resolved `--model provider/id`."""
    ref = sys.argv[sys.argv.index("--model") + 1] if "--model" in sys.argv else ""
    provider, _, model_id = ref.partition("/")
    return {"provider": provider, "id": model_id} if model_id else None


session_dir = Path(sys.argv[sys.argv.index("--session-dir") + 1])
session_dir.mkdir(parents=True, exist_ok=True)
session_file = None


def emit(event):
    print(json.dumps(event, ensure_ascii=False), flush=True)


def tool(name, *args):
    result = subprocess.run([os.environ["RRSS_TOOL"], "tool", name, *args],
                            capture_output=True, text=True, check=True, timeout=20)
    return json.loads(result.stdout)


for line in sys.stdin:
    command = json.loads(line)
    kind = command["type"]
    response = {"type": "response", "id": command.get("id"), "command": kind, "success": True}
    try:
        if kind == "new_session":
            session_file = session_dir / f"fake-{uuid.uuid4()}.jsonl"
            session_file.write_text("0", encoding="utf-8")
            response["data"] = {"cancelled": False}
        elif kind == "switch_session":
            session_file = Path(command["sessionPath"])
            assert session_file.is_file(), "saved session missing"
            response["data"] = {"cancelled": False}
        elif kind == "get_state":
            response["data"] = {"model": current_model(), "sessionFile": str(session_file) if session_file else None, "isStreaming": False}
        elif kind == "get_available_thinking_levels":
            response["data"] = {"levels": ["off"]}
        elif kind == "prompt":
            if command["message"].startswith("拒绝发送"):
                raise ValueError("fixture rejects prompt before generation")
            match = re.search(r'"digest_id":\s*(\d+)', command["message"])
            assert match, "digest anchor missing from prompt"
            digest = tool("get_digest", "--id", match.group(1))
            item = digest["items"][1]
            assert item["number"] == 2 and item["article_ids"] == [72], "wrong second item mapping"
            article = tool("get_article", "--id", str(item["article_ids"][0]))
            turn = int(session_file.read_text(encoding="utf-8")) + 1
            session_file.write_text(str(turn), encoding="utf-8")
            reply = f"第 {turn} 轮：第二条对应 [{article['title']}](rrss://article/{article['id']})。"
            emit(response)
            emit({"type": "agent_start"})
            emit({"type": "message_end", "message": {"role": "user", "content": command["message"]}})
            emit({"type": "message_end", "message": {"role": "assistant", "content": "先读取该期的第二条来源。"}})
            emit({"type": "tool_execution_start", "toolName": "get_article"})
            emit({"type": "message_end", "message": {"role": "toolResult", "content": "工具结果不是助手回复"}})
            emit({"type": "tool_execution_end", "toolName": "get_article", "isError": False})
            for text in (reply[:5], reply[5:]):
                emit({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "delta": text}})
            emit({"type": "message_end", "message": {"role": "assistant", "content": reply}})
            emit({"type": "agent_settled"})
            continue
        elif kind == "abort":
            emit({"type": "agent_settled"})
        emit(response)
    except Exception as error:
        response.update(success=False, error=str(error))
        emit(response)
