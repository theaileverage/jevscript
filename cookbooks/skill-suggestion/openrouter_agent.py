#!/usr/bin/env python3
"""One-turn measured skill-loading agent backed by OpenRouter."""

from __future__ import annotations

import argparse
import json
import os
import ssl
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

import generate_roster
import runner
from project_env import load_project_env


DEFAULT_MODEL = "deepseek/deepseek-v4.1-flash"
ENDPOINT = "https://openrouter.ai/api/v1/chat/completions"


def _tls_context() -> ssl.SSLContext:
    """Use Python's trust store, with common verified system-CA fallbacks."""
    paths = ssl.get_default_verify_paths()
    if paths.cafile and Path(paths.cafile).is_file():
        return ssl.create_default_context()
    for fallback in (Path("/etc/ssl/cert.pem"), Path("/etc/ssl/certs/ca-certificates.crt")):
        if fallback.is_file():
            return ssl.create_default_context(cafile=fallback)
    return ssl.create_default_context()


def system_prompt(suggestion: str) -> str:
    """Render the pinned licensed roster plus the selected benchmark arm's hint."""
    return generate_roster.render_prompt(runner.load_roster()) + suggestion


def request_payload(request: str, suggestion: str, model: str) -> dict[str, Any]:
    names = [skill["name"] for skill in runner.load_roster()]
    return {
        "model": model,
        "temperature": 0,
        "messages": [
            {"role": "system", "content": system_prompt(suggestion)},
            {"role": "user", "content": request},
        ],
        "tools": [
            {
                "type": "function",
                "function": {
                    "name": "skill_view",
                    "description": "Load one relevant skill before answering the user.",
                    "parameters": {
                        "type": "object",
                        "properties": {"name": {"type": "string", "enum": names}},
                        "required": ["name"],
                        "additionalProperties": False,
                    },
                },
            }
        ],
        "tool_choice": "auto",
    }


def _loaded_skills(response: dict[str, Any]) -> list[str]:
    try:
        calls = response["choices"][0]["message"].get("tool_calls") or []
    except (KeyError, IndexError, TypeError) as exc:
        raise ValueError("OpenRouter response has no assistant message") from exc
    loaded: list[str] = []
    for call in calls:
        function = call.get("function") or {}
        if function.get("name") != "skill_view":
            continue
        arguments = function.get("arguments", "{}")
        if isinstance(arguments, str):
            arguments = json.loads(arguments)
        name = arguments.get("name") if isinstance(arguments, dict) else None
        if isinstance(name, str) and name not in loaded:
            loaded.append(name)
    return loaded


def run(request: str, suggestion: str, model: str, timeout: float) -> dict[str, Any]:
    load_project_env(runner.REPO)
    api_key = os.environ.get("OPENROUTER_API_KEY")
    if not api_key:
        raise SystemExit("OPENROUTER_API_KEY is required for a live agent turn")
    payload = json.dumps(request_payload(request, suggestion, model)).encode()
    http_request = urllib.request.Request(
        ENDPOINT,
        data=payload,
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
            "X-Title": "Jevscript skill suggestion cookbook",
        },
        method="POST",
    )
    started = time.perf_counter()
    try:
        with urllib.request.urlopen(
            http_request, timeout=timeout, context=_tls_context()
        ) as response:
            result = json.load(response)
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"OpenRouter returned HTTP {exc.code}: {detail}") from exc
    elapsed = time.perf_counter() - started
    return {
        "loaded": _loaded_skills(result),
        "model": result.get("model", model),
        "usage": result.get("usage", {}),
        "latency_seconds": round(elapsed, 3),
        "response_id": result.get("id"),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument("--timeout", type=float, default=180)
    args = parser.parse_args()
    item = json.load(sys.stdin)
    result = run(
        request=str(item["request"]),
        suggestion=str(item.get("suggestion") or ""),
        model=args.model,
        timeout=args.timeout,
    )
    print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
