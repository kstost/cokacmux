#!/usr/bin/env python3
"""Measure native agent session artifacts and compare their shapes.

This tool is intentionally read-mostly. The `snapshot`, `diff`, and
`fingerprint` commands do not modify agent storage. The `run` command invokes
one agent with a small prompt and records before/after snapshots so newly
created artifacts can be inspected and compared.
"""

from __future__ import annotations

import argparse
import collections
import datetime as _dt
import hashlib
import json
import os
import re
import sqlite3
import subprocess
import sys
import time
import uuid
from pathlib import Path
from typing import Any


UUID_RE = re.compile(
    r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"
)


def home() -> Path:
    return Path.home()


def now_tag() -> str:
    return _dt.datetime.now(_dt.UTC).strftime("%Y%m%dT%H%M%SZ")


def file_record(provider: str, path: Path, session_id: str | None = None) -> dict[str, Any]:
    stat = path.stat()
    return {
        "provider": provider,
        "session_id": session_id or infer_session_id(path),
        "path": str(path),
        "size": stat.st_size,
        "mtime_ns": stat.st_mtime_ns,
        "sha256_16": sha256_16(path),
    }


def sha256_16(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()[:16]


def infer_session_id(path: Path) -> str:
    stem = path.stem
    matches = UUID_RE.findall(stem)
    if matches:
        return matches[-1]
    return stem


def snapshot() -> dict[str, Any]:
    return {
        "created_at": _dt.datetime.now(_dt.UTC).isoformat(timespec="seconds"),
        "home": str(home()),
        "claude": snapshot_claude(),
        "codex": snapshot_codex(),
        "opencode": snapshot_opencode(),
    }


def snapshot_claude() -> list[dict[str, Any]]:
    root = home() / ".claude" / "projects"
    if not root.is_dir():
        return []
    return sorted(
        [file_record("claude", path) for path in root.rglob("*.jsonl") if path.is_file()],
        key=lambda item: item["mtime_ns"],
        reverse=True,
    )


def snapshot_codex() -> list[dict[str, Any]]:
    root = home() / ".codex" / "sessions"
    if not root.is_dir():
        return []
    return sorted(
        [file_record("codex", path) for path in root.rglob("*.jsonl") if path.is_file()],
        key=lambda item: item["mtime_ns"],
        reverse=True,
    )


def snapshot_opencode() -> list[dict[str, Any]]:
    db = home() / ".local" / "share" / "opencode" / "opencode.db"
    if not db.is_file():
        return []
    try:
        conn = sqlite3.connect(str(db))
        rows = conn.execute(
            "SELECT id, directory, title, time_updated FROM session ORDER BY time_updated DESC"
        ).fetchall()
    except sqlite3.Error:
        return []
    finally:
        try:
            conn.close()
        except Exception:
            pass
    return [
        {
            "provider": "opencode",
            "session_id": row[0],
            "path": f"{db}#{row[0]}",
            "cwd": row[1],
            "title": row[2],
            "mtime_ns": int(row[3]) * 1_000_000,
        }
        for row in rows
    ]


def diff_snapshots(before: dict[str, Any], after: dict[str, Any]) -> dict[str, Any]:
    result: dict[str, Any] = {"new": {}, "changed": {}}
    for provider in ("claude", "codex", "opencode"):
        before_by_path = {item["path"]: item for item in before.get(provider, [])}
        after_by_path = {item["path"]: item for item in after.get(provider, [])}
        new_items = [item for path, item in after_by_path.items() if path not in before_by_path]
        changed = [
            item
            for path, item in after_by_path.items()
            if path in before_by_path
            and item.get("mtime_ns") != before_by_path[path].get("mtime_ns")
        ]
        result["new"][provider] = sorted(new_items, key=lambda item: item["mtime_ns"], reverse=True)
        result["changed"][provider] = sorted(
            changed, key=lambda item: item["mtime_ns"], reverse=True
        )
    return result


def fingerprint(provider: str, artifact: str) -> dict[str, Any]:
    if provider in ("claude", "codex"):
        return jsonl_fingerprint(provider, Path(artifact))
    if provider == "opencode":
        if "#" in artifact:
            db, session_id = artifact.split("#", 1)
        else:
            db, session_id = artifact, ""
        return opencode_fingerprint(Path(db), session_id)
    raise SystemExit(f"unknown provider: {provider}")


def shape(provider: str, artifact: str) -> dict[str, Any]:
    if provider in ("claude", "codex"):
        return jsonl_shape(provider, Path(artifact))
    if provider == "opencode":
        if "#" in artifact:
            db, session_id = artifact.split("#", 1)
        else:
            db, session_id = artifact, ""
        return opencode_shape(Path(db), session_id)
    raise SystemExit(f"unknown provider: {provider}")


def value_shape(value: Any, depth: int = 0, max_depth: int = 5) -> Any:
    """Return a redacted structural summary of JSON-like data.

    Strings are not emitted verbatim. This lets us compare native session
    structure without leaking prompts, tool outputs, paths beyond the table row
    columns, or other user content.
    """

    if depth >= max_depth:
        return type_name(value)
    if isinstance(value, dict):
        return {
            "object": {
                key: value_shape(value[key], depth + 1, max_depth)
                for key in sorted(value.keys())
            }
        }
    if isinstance(value, list):
        shapes = []
        seen = set()
        for item in value:
            item_shape = value_shape(item, depth + 1, max_depth)
            key = json.dumps(item_shape, sort_keys=True, ensure_ascii=False)
            if key not in seen:
                seen.add(key)
                shapes.append(item_shape)
        return {"array": shapes}
    return type_name(value)


def type_name(value: Any) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "bool"
    if isinstance(value, int) and not isinstance(value, bool):
        return "integer"
    if isinstance(value, float):
        return "number"
    if isinstance(value, str):
        if UUID_RE.fullmatch(value):
            return "uuid-string"
        if re.fullmatch(r"(ses|msg|prt|evt)_[0-9a-f]{12}[0-9A-Za-z]{14}", value):
            return "opencode-id-string"
        if value.startswith("/") or value.startswith("~"):
            return "path-string"
        if value.startswith("{") or value.startswith("["):
            return "json-string"
        return "string"
    return type(value).__name__


def signature(value: Any) -> str:
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"))


def jsonl_shape(provider: str, path: Path) -> dict[str, Any]:
    lines = []
    signature_counts: collections.Counter[str] = collections.Counter()
    parse_errors = 0
    with path.open("r", encoding="utf-8", errors="replace") as f:
        for line_no, raw in enumerate(f, 1):
            line = raw.strip()
            if not line:
                continue
            try:
                value = json.loads(line)
            except json.JSONDecodeError:
                parse_errors += 1
                lines.append({"line": line_no, "parse_error": True})
                continue
            if not isinstance(value, dict):
                parse_errors += 1
                lines.append({"line": line_no, "non_object": type_name(value)})
                continue
            typ = value.get("type")
            payload = value.get("payload")
            message = value.get("message")
            entry = {
                "line": line_no,
                "type": typ,
                "top_keys": sorted(value.keys()),
                "payload_type": payload.get("type") if isinstance(payload, dict) else None,
                "payload_shape": value_shape(payload) if isinstance(payload, dict) else type_name(payload),
                "message_role": message.get("role") if isinstance(message, dict) else None,
                "message_shape": value_shape(message) if isinstance(message, dict) else type_name(message),
            }
            lines.append(entry)
            signature_counts[signature({k: v for k, v in entry.items() if k != "line"})] += 1
    return {
        "provider": provider,
        "artifact": str(path),
        "session_id": infer_session_id(path),
        "line_count": len(lines),
        "parse_errors": parse_errors,
        "lines": lines,
        "signature_counts": [
            {"count": count, "shape": json.loads(sig)}
            for sig, count in signature_counts.most_common()
        ],
    }


def opencode_shape(db: Path, session_id: str) -> dict[str, Any]:
    conn = sqlite3.connect(str(db))
    conn.row_factory = sqlite3.Row
    try:
        session_rows = conn.execute(
            "SELECT * FROM session WHERE id = ?", (session_id,)
        ).fetchall()
        message_rows = conn.execute(
            "SELECT * FROM message WHERE session_id = ? ORDER BY time_created, id",
            (session_id,),
        ).fetchall() if table_exists(conn, "message") else []
        part_rows = conn.execute(
            "SELECT * FROM part WHERE session_id = ? ORDER BY time_created, id",
            (session_id,),
        ).fetchall() if table_exists(conn, "part") else []
        session_message_rows = conn.execute(
            "SELECT * FROM session_message WHERE session_id = ? ORDER BY time_created, id",
            (session_id,),
        ).fetchall() if table_exists(conn, "session_message") else []
        return {
            "provider": "opencode",
            "artifact": f"{db}#{session_id}",
            "session_id": session_id,
            "session": [opencode_row_shape(row, parse_json_columns={"model", "permission"}) for row in session_rows],
            "messages": [opencode_row_shape(row, parse_json_columns={"data"}) for row in message_rows],
            "parts": [opencode_row_shape(row, parse_json_columns={"data"}) for row in part_rows],
            "session_messages": [
                opencode_row_shape(row, parse_json_columns={"data"}) for row in session_message_rows
            ],
            "aggregate": {
                "message_data_shapes": aggregate_json_shapes(message_rows, "data", "role"),
                "part_data_shapes": aggregate_json_shapes(part_rows, "data", "type"),
                "session_message_data_shapes": aggregate_json_shapes(
                    session_message_rows, "data", None, row_type_column="type"
                ),
            },
        }
    finally:
        conn.close()


def opencode_row_shape(row: sqlite3.Row, parse_json_columns: set[str]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for key in row.keys():
        value = row[key]
        if key in parse_json_columns and isinstance(value, str):
            try:
                out[key] = value_shape(json.loads(value))
            except Exception:
                out[key] = type_name(value)
        else:
            out[key] = type_name(value)
    return out


def aggregate_json_shapes(
    rows: list[sqlite3.Row],
    json_column: str,
    discriminator_key: str | None,
    row_type_column: str | None = None,
) -> list[dict[str, Any]]:
    counter: collections.Counter[str] = collections.Counter()
    labels: dict[str, str] = {}
    for row in rows:
        try:
            data = json.loads(row[json_column])
        except Exception:
            data = None
        if row_type_column:
            label = str(row[row_type_column])
        elif isinstance(data, dict) and discriminator_key:
            label = str(data.get(discriminator_key, ""))
        else:
            label = ""
        item = {"label": label, "shape": value_shape(data)}
        sig = signature(item)
        labels[sig] = label
        counter[sig] += 1
    return [
        {"count": count, **json.loads(sig)}
        for sig, count in counter.most_common()
    ]


def jsonl_fingerprint(provider: str, path: Path) -> dict[str, Any]:
    type_counts: collections.Counter[str] = collections.Counter()
    payload_type_counts: collections.Counter[str] = collections.Counter()
    top_keys_by_type: dict[str, list[str]] = {}
    content_block_types: collections.Counter[str] = collections.Counter()
    line_count = 0
    parse_errors = 0
    first_session_id = None
    first_cwd = None
    with path.open("r", encoding="utf-8", errors="replace") as f:
        for raw in f:
            line = raw.strip()
            if not line:
                continue
            line_count += 1
            try:
                value = json.loads(line)
            except json.JSONDecodeError:
                parse_errors += 1
                continue
            if not isinstance(value, dict):
                parse_errors += 1
                continue
            typ = str(value.get("type", ""))
            type_counts[typ] += 1
            top_keys_by_type.setdefault(typ, list(value.keys()))
            first_session_id = first_session_id or value.get("sessionId")
            first_cwd = first_cwd or value.get("cwd")
            payload = value.get("payload")
            if isinstance(payload, dict):
                payload_type_counts[str(payload.get("type", ""))] += 1
                first_session_id = first_session_id or payload.get("id")
                first_cwd = first_cwd or payload.get("cwd")
            message = value.get("message")
            if isinstance(message, dict):
                content = message.get("content")
                if isinstance(content, list):
                    for block in content:
                        if isinstance(block, dict):
                            content_block_types[str(block.get("type", ""))] += 1
    return {
        "provider": provider,
        "artifact": str(path),
        "session_id": infer_session_id(path),
        "line_count": line_count,
        "parse_errors": parse_errors,
        "first_session_id": first_session_id,
        "first_cwd": first_cwd,
        "type_counts": dict(type_counts),
        "payload_type_counts": dict(payload_type_counts),
        "content_block_types": dict(content_block_types),
        "top_keys_by_type": top_keys_by_type,
    }


def opencode_fingerprint(db: Path, session_id: str) -> dict[str, Any]:
    conn = sqlite3.connect(str(db))
    try:
        tables = {
            name: conn.execute(f"PRAGMA table_info({name})").fetchall()
            for name in ("session", "message", "part", "session_message", "project")
            if table_exists(conn, name)
        }
        counts = {}
        for name in ("session", "message", "part", "session_message"):
            if table_exists(conn, name):
                if name == "session":
                    sql = "SELECT COUNT(*) FROM session WHERE id = ?"
                else:
                    sql = f"SELECT COUNT(*) FROM {name} WHERE session_id = ?"
                counts[name] = conn.execute(sql, (session_id,)).fetchone()[0]
        data_key_counts = {}
        for name in ("message", "part", "session_message"):
            if table_exists(conn, name):
                rows = conn.execute(
                    f"SELECT data FROM {name} WHERE session_id = ? LIMIT 50", (session_id,)
                ).fetchall()
                key_counter: collections.Counter[str] = collections.Counter()
                for (text,) in rows:
                    try:
                        value = json.loads(text)
                    except Exception:
                        continue
                    if isinstance(value, dict):
                        key_counter.update(value.keys())
                data_key_counts[name] = dict(key_counter)
        return {
            "provider": "opencode",
            "artifact": f"{db}#{session_id}",
            "session_id": session_id,
            "tables": {name: [col[1] for col in cols] for name, cols in tables.items()},
            "counts": counts,
            "data_key_counts": data_key_counts,
        }
    finally:
        conn.close()


def table_exists(conn: sqlite3.Connection, name: str) -> bool:
    row = conn.execute(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?", (name,)
    ).fetchone()
    return bool(row and row[0])


def run_probe(args: argparse.Namespace) -> dict[str, Any]:
    bundle = Path(args.out or f"native_probe_{args.agent}_{now_tag()}")
    bundle.mkdir(parents=True, exist_ok=True)
    before = snapshot()
    write_json(bundle / "before.json", before)
    started = time.time()
    command = agent_command(args, bundle)
    completed = subprocess.run(
        command,
        cwd=args.cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=args.timeout,
    )
    (bundle / "stdout.txt").write_text(completed.stdout, encoding="utf-8")
    (bundle / "stderr.txt").write_text(completed.stderr, encoding="utf-8")
    after = snapshot()
    write_json(bundle / "after.json", after)
    delta = diff_snapshots(before, after)
    write_json(bundle / "diff.json", delta)
    fingerprints = []
    for provider, items in delta["new"].items():
        for item in items[:5]:
            try:
                fingerprints.append(fingerprint(provider, item["path"]))
            except Exception as exc:
                fingerprints.append(
                    {
                        "provider": provider,
                        "artifact": item["path"],
                        "error": str(exc),
                    }
                )
    write_json(bundle / "fingerprints.json", fingerprints)
    report = {
        "bundle": str(bundle),
        "agent": args.agent,
        "command": command,
        "returncode": completed.returncode,
        "duration_s": round(time.time() - started, 3),
        "new_counts": {provider: len(items) for provider, items in delta["new"].items()},
        "changed_counts": {provider: len(items) for provider, items in delta["changed"].items()},
    }
    write_json(bundle / "report.json", report)
    return report


def agent_command(args: argparse.Namespace, bundle: Path) -> list[str]:
    prompt = args.prompt
    if args.agent == "claude":
        session_id = str(uuid.uuid4())
        return [
            "claude",
            "-p",
            "--output-format",
            "text",
            "--session-id",
            session_id,
            "--name",
            f"native-probe-{now_tag()}",
            "--max-budget-usd",
            str(args.max_budget_usd),
            prompt,
        ]
    if args.agent == "codex":
        return [
            "codex",
            "exec",
            "--skip-git-repo-check",
            "--color",
            "never",
            "-C",
            args.cwd,
            "--output-last-message",
            str(bundle / "last_message.txt"),
            prompt,
        ]
    if args.agent == "opencode":
        return [
            "opencode",
            "run",
            "--dir",
            args.cwd,
            "--title",
            f"native-probe-{now_tag()}",
            "--format",
            "json",
            prompt,
        ]
    raise SystemExit(f"unknown agent: {args.agent}")


def write_json(path: Path, value: Any) -> None:
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def read_json(path: str) -> Any:
    return json.loads(Path(path).read_text(encoding="utf-8"))


def main() -> None:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("snapshot")
    p.add_argument("--out")

    p = sub.add_parser("diff")
    p.add_argument("before")
    p.add_argument("after")

    p = sub.add_parser("fingerprint")
    p.add_argument("provider", choices=["claude", "codex", "opencode"])
    p.add_argument("artifact")
    p.add_argument("--out")

    p = sub.add_parser("shape")
    p.add_argument("provider", choices=["claude", "codex", "opencode"])
    p.add_argument("artifact")
    p.add_argument("--out")

    p = sub.add_parser("run")
    p.add_argument("agent", choices=["claude", "codex", "opencode"])
    p.add_argument("prompt")
    p.add_argument("--cwd", default=os.getcwd())
    p.add_argument("--out")
    p.add_argument("--timeout", type=int, default=180)
    p.add_argument("--max-budget-usd", type=float, default=0.05)

    args = parser.parse_args()
    if args.cmd == "snapshot":
        value = snapshot()
        if args.out:
            write_json(Path(args.out), value)
        print(json.dumps(value, ensure_ascii=False, indent=2))
    elif args.cmd == "diff":
        print(json.dumps(diff_snapshots(read_json(args.before), read_json(args.after)), ensure_ascii=False, indent=2))
    elif args.cmd == "fingerprint":
        value = fingerprint(args.provider, args.artifact)
        if args.out:
            write_json(Path(args.out), value)
        print(json.dumps(value, ensure_ascii=False, indent=2))
    elif args.cmd == "shape":
        value = shape(args.provider, args.artifact)
        if args.out:
            write_json(Path(args.out), value)
        print(json.dumps(value, ensure_ascii=False, indent=2))
    elif args.cmd == "run":
        print(json.dumps(run_probe(args), ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
