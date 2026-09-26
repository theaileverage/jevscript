"""Durable, scoped conversation threads and incoming-message receipts.

The Jevscript route is authoritative. This store validates identities, supplies
bounded same-scope candidates, and applies a decision once under the home lock.
"""

from __future__ import annotations

import hashlib
import json
import os
import secrets
import tempfile
from pathlib import Path
from typing import Any

from .home import Home, iso, read_json, write_json

CANDIDATE_LIMIT = 8
RECENT_LIMIT = 8


def _id(value: str | None, name: str) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str) or not value or len(value) > 200 or any(ord(c) < 32 for c in value):
        raise ValueError(f"invalid {name}")
    return value


class Threads:
    """Section 6.4a candidates backed by host-owned durable message correlation."""

    def __init__(self, home: Home) -> None:
        self.home = home
        self.root = home.data / "threads"
        self.receipts = self.root / "messages"
        self.root.mkdir(parents=True, exist_ok=True)
        self.receipts.mkdir(parents=True, exist_ok=True)

    def scope(self, request: dict[str, Any]) -> dict[str, str]:
        return {
            "source": _id(request.get("source"), "source") or "cos",
            "project": _id(request.get("effective_project") or request.get("project"), "project") or "",
            "channel": _id(request.get("channel"), "channel") or "",
        }

    def receipt_path(self, scope: dict[str, str], message_id: str) -> Path:
        key = json.dumps([scope, _id(message_id, "message_id")], sort_keys=True)
        return self.receipts / (hashlib.sha256(key.encode()).hexdigest() + ".json")

    def receipt(self, scope: dict[str, str], message_id: str | None) -> dict[str, Any] | None:
        return read_json(self.receipt_path(scope, message_id), None) if message_id else None

    def enqueue(self, request: dict[str, Any]) -> dict[str, Any]:
        """Reserve a scoped provider ID before writing a request file.

        Exclusive creation deduplicates concurrent delivery even if the host is
        asleep. A reserved receipt is retried from its original request ID.
        """
        scope = self.scope(request)
        message_id = _id(request.get("message_id"), "message_id")
        if not message_id:
            raise ValueError("message_id is required")
        _id(request.get("reply_to"), "reply_to")
        _id(request.get("native_thread"), "native_thread")
        path = self.receipt_path(scope, message_id)
        record = {"scope": scope, "message_id": message_id, "request_id": request["id"], "request": request, "status": "pending"}
        fd, temp = tempfile.mkstemp(prefix=".incoming-", dir=self.receipts)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as handle:
                json.dump(record, handle)
            try:
                os.link(temp, path)
            except FileExistsError:
                pass
            else:
                return record
        finally:
            os.unlink(temp)
        if path.exists():
            existing = read_json(path, record)
            original = existing.get("request", {})
            if any(original.get(field) != request.get(field) for field in ("text", "reply_to", "native_thread")):
                raise ValueError("message_id was already used for a different message")
            return existing
        raise RuntimeError("message reservation disappeared")

    def get(self, thread_id: str) -> dict[str, Any] | None:
        if not isinstance(thread_id, str) or not thread_id.startswith("th-") or "/" in thread_id:
            return None
        return read_json(self.root / f"{thread_id}.json", None)

    def all(self) -> list[dict[str, Any]]:
        return [read_json(path, {}) for path in self.root.glob("th-*.json")]

    def relation(self, request: dict[str, Any]) -> dict[str, Any]:
        scope = self.scope(request)
        missing = None
        for field in ("native_thread", "reply_to"):
            value = request.get(field)
            if not value:
                continue
            if field == "native_thread" and value == request.get("message_id"):
                continue
            if field == "native_thread":
                found = next((t for t in self.all() if t.get("scope") == scope and t.get("native_thread") == value), None)
                thread_id = found["id"] if found else None
            else:
                receipt = self.receipt(scope, value)
                thread_id = receipt.get("thread_id") if receipt else None
            thread = self.get(thread_id) if thread_id else None
            if thread and thread.get("scope") == scope:
                return {"kind": "valid", "thread_id": thread_id, "task_id": thread.get("task_id"), "reason": field}
            missing = field
        if missing:
            return {"kind": "missing", "thread_id": None, "reason": missing}
        return {"kind": "none", "thread_id": None, "reason": ""}

    def candidates(self, request: dict[str, Any]) -> list[dict[str, str]]:
        scope = self.scope(request)
        rows = [t for t in self.all() if t.get("scope") == scope and not any(m["id"] == request.get("message_id") for m in t.get("messages", []))]
        rows.sort(key=lambda t: t.get("updated", ""), reverse=True)
        return [{"id": t["id"], "task_id": t.get("task_id"), "summary": t.get("summary", "")[:600]} for t in rows[:CANDIDATE_LIMIT]]

    def apply(self, request: dict[str, Any], decision: dict[str, Any], task_id: str | None = None) -> dict[str, Any]:
        scope = self.scope(request)
        original_scope = self.scope({**request, "effective_project": None})
        message_id = request["message_id"]
        receipt = self.receipt(scope, message_id) or self.receipt(original_scope, message_id)
        if receipt and receipt.get("status") == "done":
            if original_scope != scope:
                write_json(self.receipt_path(original_scope, message_id), receipt)
            return receipt
        route = decision["route"]
        thread = self.get(decision.get("thread_id")) if route == "continue" else None
        if route == "continue" and (thread is None or thread["scope"] != scope):
            raise ValueError("thread decision escaped its scope")
        if route == "new":
            thread = next((t for t in self.all() if t.get("scope") == scope and any(m["id"] == message_id for m in t.get("messages", []))), None)
            if thread is None:
                thread = {"id": f"th-{secrets.token_hex(10)}", "scope": scope, "native_thread": request.get("native_thread"), "messages": [], "summary": "", "task_id": None}
        if thread:
            if not any(m["id"] == message_id for m in thread["messages"]):
                thread["messages"].append({"id": message_id, "request_id": request["id"], "text": request["text"], "at": request["at"]})
            thread["summary"] = "\n".join(m["text"] for m in thread["messages"][-RECENT_LIMIT:])[:600]
            thread["updated"] = iso()
            if task_id:
                thread["task_id"] = task_id
            write_json(self.root / f"{thread['id']}.json", thread)
        result = {**(receipt or {}), "scope": scope, "status": "done", "route": route, "reason": decision.get("reason", ""), "thread_id": thread["id"] if thread else None, "task_id": thread.get("task_id") if thread else None}
        write_json(self.receipt_path(scope, message_id), result)
        if original_scope != scope:
            write_json(self.receipt_path(original_scope, message_id), result)
        return result
