#!/usr/bin/env python3
"""Scripted fake Pi RPC for rrss-app L2 tests. No model and no network.

The scenario is chosen by a marker in the prompt text:
  [[model-error]]  assistant message ends with stopReason "error"
  [[retry-fail]]   auto retry ends with success=false
  [[exit]]         stream half a sentence, then exit mid-turn
  [[slow]]         stream slowly until an abort command arrives
  [[confirm:N]]    ask an extension confirm with an N ms timeout, answer by the result
  [[select]]       ask an extension select; the host must cancel it
  [[tools]]        one tool call before the answer
  [[reject]]       reject the prompt command itself (preflight failure)
  anything else    a short plain answer

Every command received is appended as one JSON line to <session-dir>/../fake-pi.log,
followed by "EOF" when stdin closes. Each start first logs one "START {...}" line with the
arguments and the environment variables RRSS sets.

Handshake: get_state reports the model from `--model provider/id` (a model id "wrong-model"
is reported under provider "openai" to simulate a model Pi did not load; without --model it
reports the user's default "user/default-model"); get_available_models lists two user models;
set_model switches in process to a model listed in the generated models.json (or a user model
without an agent dir) and get_state reports it afterwards; get_available_thinking_levels reads
models.json in PI_CODING_AGENT_DIR: a model with "reasoning": true supports off..medium,
otherwise only off.
"""
import json
import os
import select
import sys
import time
import uuid
from pathlib import Path

if "--version" in sys.argv:
    print("1.5.0")
    sys.exit(0)

session_dir = Path(sys.argv[sys.argv.index("--session-dir") + 1])
session_dir.mkdir(parents=True, exist_ok=True)
log_path = session_dir.parent / "fake-pi.log"
session_file = None
buf = b""
timed_out_prompt = False

# Turn machinery driven by the loop below.
waiting_ui = None      # (id, deadline, kind)
streaming = None       # {"text": str, "left": int, "next": float}


def log(line):
    with open(log_path, "a", encoding="utf-8") as f:
        f.write(line + "\n")


def arg(name):
    return sys.argv[sys.argv.index(name) + 1] if name in sys.argv else None


ENV_KEYS = ["PI_CODING_AGENT_DIR", "PI_OFFLINE", "PI_SKIP_VERSION_CHECK",
            "RRSS_TOOL_MANIFEST", "OPENAI_BASE_URL", "OPENAI_API_KEY",
            "HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy",
            "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy"]
ENV = {k: os.environ.get(k) for k in ENV_KEYS}
ENV.update({k: v for k, v in os.environ.items() if k.startswith("RRSS_PI_KEY")})
log("START " + json.dumps({"args": sys.argv[1:], "env": ENV}, ensure_ascii=False))


USER_MODELS = [{"provider": "user", "id": "default-model"}, {"provider": "user", "id": "other-model"}]


def current_model():
    ref = arg("--model")
    if ref is None:
        # No --model: the user's own Pi setup picks its default model.
        return USER_MODELS[0]
    provider, _, model_id = ref.partition("/")
    if model_id == "wrong-model":
        provider = "openai"
    return {"provider": provider, "id": model_id} if model_id else None


def generated_models():
    agent = os.environ.get("PI_CODING_AGENT_DIR")
    try:
        providers = json.loads((Path(agent) / "models.json").read_text(encoding="utf-8"))["providers"]
    except Exception:
        return None
    return [{"provider": name, "id": m["id"], "reasoning": m.get("reasoning", False), "contextWindow": m.get("contextWindow", 32000)}
            for name, p in providers.items() for m in p.get("models", [])]


def thinking_levels():
    models = generated_models() or []
    reasoning = next((m["reasoning"] for m in models if m["provider"] == state_model["provider"] and m["id"] == state_model["id"]), False)
    return ["off", "minimal", "low", "medium"] if reasoning else ["off"]


state_model = current_model()


def emit(event):
    sys.stdout.write(json.dumps(event, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def respond(command, success=True, data=None, error=None):
    event = {"type": "response", "id": command.get("id"), "command": command.get("type"), "success": success}
    if data is not None:
        event["data"] = data
    if error is not None:
        event["error"] = error
    emit(event)


def assistant(text, stop="stop", error=None):
    message = {"role": "assistant", "content": [{"type": "text", "text": text}] if text else [], "stopReason": stop}
    if error:
        message["errorMessage"] = error
    message["usage"] = {"input": 100, "output": 20, "cacheRead": 30, "cacheWrite": 40}
    emit({"type": "message_end", "message": message})


def settle():
    emit({"type": "agent_end", "messages": [], "willRetry": False})
    emit({"type": "agent_settled"})


def answer(text):
    for piece in (text[: len(text) // 2], text[len(text) // 2:]):
        emit({"type": "message_update", "usage": {"input": 1, "output": 1},
              "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": piece}})
    assistant(text)
    settle()


context_tokens = 250


def start_prompt(message):
    global waiting_ui, streaming, context_tokens
    context_tokens = None if "[[usage-fallback]]" in message else 250
    emit({"type": "agent_start"})
    emit({"type": "message_end", "message": {"role": "user", "content": message}})
    if "[[model-error]]" in message:
        assistant("", "error", "401 Incorrect API key provided: sk-abcdef123456")
        settle()
    elif "[[retry-fail]]" in message:
        emit({"type": "auto_retry_start", "attempt": 1, "maxAttempts": 2, "delayMs": 10, "errorMessage": "529 overloaded"})
        assistant("", "error", "529 overloaded")
        emit({"type": "auto_retry_end", "success": False, "attempt": 2, "finalError": "529 overloaded_error: Overloaded"})
        settle()
    elif "[[compact-ok]]" in message or "[[compact-unknown]]" in message:
        emit({"type": "compaction_start", "reason": "threshold"})
        context_tokens = None if "[[compact-unknown]]" in message else 77
        emit({"type": "compaction_end", "aborted": False, "result": {"summary": "摘要"}})
        answer("压缩后的回答。")
    elif "[[compact" in message:
        emit({"type": "compaction_start", "reason": "threshold"})
        emit({"type": "compaction_end", "aborted": "[[compact-abort]]" in message,
              "errorMessage": "401 compaction key sk-compactsecret"})
        answer("已收到的正文。")
    elif "[[exit]]" in message:
        emit({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "delta": "半句"}})
        time.sleep(0.05)
        os._exit(3)
    elif "[[slow]]" in message:
        streaming = {"text": "", "left": 400, "next": time.monotonic()}
    elif "[[confirm:" in message:
        timeout = int(message.split("[[confirm:", 1)[1].split("]]", 1)[0])
        emit({"type": "tool_execution_start", "toolCallId": "call_fetch", "toolName": "fetch_url", "args": {"url": "https://example.org"}})
        ui = f"ui-{uuid.uuid4().hex[:8]}"
        emit({"type": "extension_ui_request", "id": ui, "method": "confirm", "title": "读取网页", "message": "执行 fetch_url？", "timeout": timeout})
        waiting_ui = (ui, time.monotonic() + timeout / 1000, "confirm")
    elif "[[select]]" in message:
        ui = f"ui-{uuid.uuid4().hex[:8]}"
        emit({"type": "extension_ui_request", "id": ui, "method": "select", "title": "Pick", "options": ["A", "B"]})
        waiting_ui = (ui, time.monotonic() + 5, "select")
    elif "[[tools]]" in message:
        emit({"type": "tool_execution_start", "toolCallId": "call_1", "toolName": "search_articles", "args": {"query": "rust"}})
        emit({"type": "tool_execution_end", "toolCallId": "call_1", "toolName": "search_articles", "result": {"content": []}, "isError": False})
        answer("查到了两篇。")
    else:
        answer("收到，这是回答。")


def finish_ui(result):
    global waiting_ui
    _, _, kind = waiting_ui
    waiting_ui = None
    if kind == "confirm":
        emit({"type": "tool_execution_end", "toolCallId": "call_fetch", "toolName": "fetch_url", "result": {"content": []}, "isError": False})
        answer("已读取网页。" if result is True else "用户取消了读取。")
    else:
        answer("select cancelled" if result == "cancelled" else "select answered")


def handle(command):
    global session_file, streaming, state_model
    kind = command.get("type")
    if kind == "extension_ui_response":
        if waiting_ui and command.get("id") == waiting_ui[0]:
            if command.get("cancelled"):
                finish_ui(False if waiting_ui[2] == "confirm" else "cancelled")
            elif "confirmed" in command:
                finish_ui(bool(command["confirmed"]))
            else:
                finish_ui(command.get("value"))
        return
    if kind == "new_session":
        session_file = session_dir / f"fake-{uuid.uuid4()}.jsonl"
        session_file.write_text("1", encoding="utf-8")
        respond(command, data={"cancelled": False})
    elif kind == "switch_session":
        path = Path(command["sessionPath"])
        if not path.is_file():
            respond(command, False, error=f"Session file not found: {path}")
        else:
            session_file = path
            respond(command, data={"cancelled": False})
    elif kind == "set_model":
        known = generated_models()
        known = USER_MODELS if known is None else known
        wanted = {"provider": command.get("provider"), "id": command.get("modelId")}
        if any(m["provider"] == wanted["provider"] and m["id"] == wanted["id"] for m in known):
            state_model = wanted
            respond(command, data=wanted)
        else:
            respond(command, False, error=f"Model not found: {wanted['provider']}/{wanted['id']}")
    elif kind == "get_session_stats":
        respond(command, data={"tokens": {"total": 99999}, "contextUsage": {"tokens": context_tokens}})
    elif kind == "get_state":
        reported = next((m for m in (generated_models() or USER_MODELS) if m["provider"] == state_model["provider"] and m["id"] == state_model["id"]), state_model)
        respond(command, data={"model": reported, "sessionFile": str(session_file) if session_file else None,
                               "isStreaming": streaming is not None})
    elif kind == "get_available_thinking_levels":
        respond(command, data={"levels": thinking_levels()})
    elif kind == "get_available_models":
        respond(command, data={"models": generated_models() or USER_MODELS})
    elif kind == "prompt":
        message = command.get("message", "")
        if "[[timeout]]" in message:
            # 不回应 prompt；stdin 关闭后仍输出一轮，验证宿主隔离迟到事件。
            global timed_out_prompt
            timed_out_prompt = True
            return
        if "[[reject]]" in message:
            respond(command, False, error="No model configured (key sk-zzzzzzzz)")
            return
        respond(command)
        start_prompt(message)
    elif kind == "abort":
        if streaming is not None:
            text = streaming["text"]
            streaming = None
            assistant(text, "aborted", "Request was aborted")
            settle()
        respond(command)
    else:
        respond(command, data={})


while True:
    now = time.monotonic()
    timeout = 0.05
    ready, _, _ = select.select([0], [], [], timeout)
    if ready:
        chunk = os.read(0, 65536)
        if not chunk:
            if timed_out_prompt:
                answer("上一轮的迟到污染")
            log("EOF")
            sys.exit(0)
        buf += chunk
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            if not line.strip():
                continue
            command = json.loads(line)
            log(json.dumps(command, ensure_ascii=False))
            handle(command)
    now = time.monotonic()
    if waiting_ui and now >= waiting_ui[1]:
        # Pi auto-resolves a timed-out dialog: confirm -> false, others -> undefined.
        finish_ui(False if waiting_ui[2] == "confirm" else None)
    if streaming is not None and now >= streaming["next"]:
        streaming["text"] += "字"
        streaming["left"] -= 1
        streaming["next"] = now + 0.02
        emit({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "delta": "字"}})
        if streaming["left"] <= 0:
            text = streaming["text"]
            streaming = None
            assistant(text)
            settle()
