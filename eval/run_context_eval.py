#!/usr/bin/env python3
"""Semantic evaluation for GRY context and lifecycle hooks.

This runner consumes the public CLI output (``gry context`` / ``gry hook``) and
compares it with a manually reviewed task fixture. It deliberately reports
precision, recall, ranking, relationship completeness, impact quality and
freshness as separate metrics: a single blended score would hide which part of
the evidence pipeline failed.

Standard library only. Run from the GRY repository root.
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable

Suite = dict[str, Any]


def norm_file(path: str) -> str:
    path = path.strip().replace("\\", "/")
    while path.startswith("./"):
        path = path[2:]
    return path


def symbol_matches(actual: str, expected: str) -> bool:
    """Suffix matching keeps expectations stable across project prefixes."""
    a = actual.strip()
    e = expected.strip()
    return a == e or a.endswith("." + e) or a.endswith(e)


def unique(items: Iterable[str]) -> list[str]:
    out: list[str] = []
    seen: set[str] = set()
    for item in items:
        key = item.strip()
        if not key or key in seen:
            continue
        seen.add(key)
        out.append(key)
    return out


def ranked_symbols(context: dict[str, Any]) -> list[str]:
    # Node order is the engine's evidence ranking. Resolved target labels are
    # already represented as nodes; listing them first would let noisy
    # low-value targets outrank the handler/callee chain.
    out: list[str] = []
    for node in context.get("nodes") or []:
        if node.get("qn"):
            out.append(node["qn"])
    for rel in context.get("relationships") or []:
        for key in ("source", "target"):
            if rel.get(key):
                out.append(rel[key])
    for target in context.get("targets") or []:
        if target.get("qn"):
            out.append(target["qn"])
    return unique(out)


def ranked_files(context: dict[str, Any]) -> list[str]:
    out: list[str] = []
    for row in context.get("files") or []:
        if row.get("path"):
            out.append(norm_file(row["path"]))
    for node in context.get("nodes") or []:
        if node.get("file"):
            out.append(norm_file(node["file"]))
    for group in ("tests", "services", "boundaries"):
        for row in context.get(group) or []:
            if row.get("file"):
                out.append(norm_file(row["file"]))
    for row in (context.get("changes") or {}).get("rows") or []:
        if row.get("path"):
            out.append(norm_file(row["path"]))
    return unique(out)


def relation_keys(context: dict[str, Any]) -> list[tuple[str, str, str]]:
    out = []
    for rel in context.get("relationships") or []:
        out.append(
            (
                str(rel.get("source", "")),
                str(rel.get("type", "")),
                str(rel.get("target", "")),
            )
        )
    return out


def relation_present(
    relations: list[tuple[str, str, str]], expected: dict[str, Any]
) -> bool:
    types = expected.get("types") or [expected.get("type", "")]
    return any(
        symbol_matches(source, expected["source"])
        and source_type.upper() in {t.upper() for t in types}
        and symbol_matches(target, expected["target"])
        for source, source_type, target in relations
    )


def precision_at(ranked: list[str], expected: list[str], k: int, match) -> float:
    top = ranked[:k]
    if not top:
        return 0.0
    hits = sum(1 for item in top if any(match(item, exp) for exp in expected))
    return hits / len(top)


def recall_at(ranked: list[str], expected: list[str], k: int, match) -> float:
    if not expected:
        return 1.0
    top = ranked[:k]
    found = sum(1 for exp in expected if any(match(item, exp) for item in top))
    return found / len(expected)


def reciprocal_rank(ranked: list[str], expected: list[str], match) -> float:
    for idx, item in enumerate(ranked, start=1):
        if any(match(item, exp) for exp in expected):
            return 1.0 / idx
    return 0.0


def file_match(actual: str, expected: str) -> bool:
    return norm_file(actual) == norm_file(expected)


def freshness_ok(task: dict[str, Any], context: dict[str, Any]) -> bool:
    status = context.get("status")
    if status not in task.get("expected_status", ["ok"]):
        return False
    want = task.get("freshness")
    if want is None:
        return True
    evidence = context.get("evidence") or {}
    freshness = evidence.get("freshness") or (context.get("freshness") or {}).get("freshness")
    if want == "fresh":
        return freshness == "fresh" and not (context.get("freshness") or {}).get("stale")
    if want == "stale":
        return freshness in ("stale", "generation_changed") or bool(
            (context.get("freshness") or {}).get("stale")
        )
    return freshness == want


def impact_rows(context: dict[str, Any]) -> list[str]:
    rows: list[str] = []
    changes = context.get("changes") or {}
    for row in changes.get("rows") or []:
        if row.get("indexed") is False and row.get("path"):
            rows.append(norm_file(row["path"]))
    # Inbound evidence is the impact set: callers, handlers and importers of
    # the changed file. Downstream callees are part of a trace, not proof that
    # changing this file forces them to change.
    for node in context.get("nodes") or []:
        role = node.get("role", "")
        if role in ("caller", "used_by", "handler", "importer", "subtype") and node.get("file"):
            rows.append(norm_file(node["file"]))
    impact_roles = {"caller", "used_by", "handler", "importer", "subtype"}
    for row in context.get("files") or []:
        if row.get("path") and set(row.get("roles") or []) & impact_roles:
            rows.append(norm_file(row["path"]))
    return unique(rows)


def run_command(task: dict[str, Any], args: argparse.Namespace) -> dict[str, Any]:
    cmd = [args.gry]
    if task["interface"] == "hook":
        cmd += [
            "hook",
            task["stage"],
            "--project",
            args.project,
            "--json",
            "--no-cache",
            "--max-tokens",
            str(args.max_tokens),
        ]
        if task.get("task"):
            cmd += ["--task", task["task"]]
        for target in task.get("targets") or []:
            cmd += ["--target", target]
        for path in task.get("changed_paths") or []:
            cmd += ["--changed-path", path]
    else:
        cmd += [
            "context",
            args.project,
            "--json",
            "--profile",
            task.get("profile", "context"),
            "--depth",
            str(task.get("depth", 2)),
            "--max-tokens",
            str(args.max_tokens),
        ]
        if task.get("task"):
            cmd += ["--task", task["task"]]
        for target in task.get("targets") or []:
            cmd += ["--target", target]
        for item in task.get("include") or []:
            cmd += ["--include", item]

    completed = subprocess.run(
        cmd,
        env=os.environ.copy(),
        capture_output=True,
        text=True,
        timeout=args.timeout,
        check=False,
    )
    if completed.returncode != 0:
        return {
            "error": f"exit {completed.returncode}",
            "stderr": completed.stderr.strip()[:2000],
            "command": cmd,
        }
    try:
        payload = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        return {"error": f"invalid JSON: {exc}", "stdout": completed.stdout[:2000], "command": cmd}
    if task["interface"] == "hook":
        context = payload.get("context")
        return {
            "hook": payload.get("hook"),
            "failure": payload.get("failure"),
            "context": context,
            "command": cmd,
        }
    return {"context": payload, "command": cmd}


def evaluate(task: dict[str, Any], result: dict[str, Any], k: int) -> dict[str, Any]:
    out: dict[str, Any] = {
        "id": task["id"],
        "category": task.get("category"),
        "holdout": bool(task.get("holdout")),
    }
    if "error" in result:
        out.update({"error": result["error"], "stderr": result.get("stderr")})
        return out
    context = result.get("context") or {}
    out["status"] = context.get("status")
    out["evidence_freshness"] = (context.get("evidence") or {}).get("freshness")
    out["complete_for_scope"] = (context.get("evidence") or {}).get("complete_for_scope")
    out["freshness_ok"] = freshness_ok(task, context)
    out["budget_truncated"] = bool((context.get("budget") or {}).get("truncated"))
    out["estimated_tokens"] = (context.get("budget") or {}).get("estimated_tokens")

    expected_files = [norm_file(p) for p in task.get("expected_files") or []]
    expected_symbols = list(task.get("expected_symbols") or [])
    files = ranked_files(context)
    symbols = ranked_symbols(context)
    out["file_precision_at_k"] = precision_at(files, expected_files, k, file_match)
    out["file_recall_at_k"] = recall_at(files, expected_files, k, file_match)
    out["file_mrr"] = reciprocal_rank(files, expected_files, file_match)
    out["symbol_precision_at_k"] = precision_at(symbols, expected_symbols, k, symbol_matches)
    out["symbol_recall_at_k"] = recall_at(symbols, expected_symbols, k, symbol_matches)
    out["symbol_mrr"] = reciprocal_rank(symbols, expected_symbols, symbol_matches)
    out["returned_files"] = len(files)
    out["returned_symbols"] = len(symbols)
    out["missing_files"] = [
        p for p in expected_files if not any(file_match(item, p) for item in files[:k])
    ]
    out["missing_symbols"] = [
        s for s in expected_symbols if not any(symbol_matches(item, s) for item in symbols[:k])
    ]

    relations = relation_keys(context)
    prov_rows = [rel.get("provenance") for rel in context.get("relationships") or []]
    known = {"verified", "declared", "inferred", "observed"}
    out["provenance_items"] = len(prov_rows)
    out["provenance_labeled"] = all(p in known for p in prov_rows) if prov_rows else True
    out["verified_relation_ratio"] = (
        sum(1 for p in prov_rows if p == "verified") / len(prov_rows) if prov_rows else 1.0
    )
    expected_relations = task.get("expected_relations") or []
    found_relations = [rel for rel in expected_relations if relation_present(relations, rel)]
    out["relation_expected"] = len(expected_relations)
    out["relation_found"] = len(found_relations)
    out["relation_recall"] = (
        len(found_relations) / len(expected_relations) if expected_relations else 1.0
    )
    out["missing_relations"] = [
        rel for rel in expected_relations if rel not in found_relations
    ]

    if task.get("impact_files"):
        expected_impact = [norm_file(p) for p in task["impact_files"]]
        reported = impact_rows(context)
        hits = [p for p in expected_impact if any(file_match(item, p) for item in reported)]
        out["impact_recall"] = len(hits) / len(expected_impact)
        out["impact_false_positive_rate"] = (
            sum(
                1
                for item in reported
                if not any(file_match(item, exp) for exp in expected_impact)
            )
            / len(reported)
            if reported
            else 0.0
        )
        out["reported_impact"] = reported[:k]

    forbidden = [norm_file(p) for p in task.get("must_not_include_files") or []]
    out["forbidden_files_returned"] = [
        p for p in files if any(file_match(p, f) for f in forbidden)
    ]
    out["failure"] = result.get("failure")
    out["command"] = result.get("command")
    return out


def macro(rows: list[dict[str, Any]], key: str) -> float | None:
    values = [row[key] for row in rows if key in row and isinstance(row[key], (int, float))]
    return statistics.fmean(values) if values else None


def summarize(rows: list[dict[str, Any]]) -> dict[str, Any]:
    ok = [row for row in rows if "error" not in row]
    return {
        "tasks": len(rows),
        "errors": len(rows) - len(ok),
        "file_precision_at_k": macro(ok, "file_precision_at_k"),
        "file_recall_at_k": macro(ok, "file_recall_at_k"),
        "file_mrr": macro(ok, "file_mrr"),
        "symbol_precision_at_k": macro(ok, "symbol_precision_at_k"),
        "symbol_recall_at_k": macro(ok, "symbol_recall_at_k"),
        "symbol_mrr": macro(ok, "symbol_mrr"),
        "relation_recall": macro(ok, "relation_recall"),
        "provenance_labeled": sum(1 for row in ok if row.get("provenance_labeled")) / len(ok)
        if ok
        else None,
        "verified_relation_ratio": macro(ok, "verified_relation_ratio"),
        "impact_recall": macro(ok, "impact_recall"),
        "impact_false_positive_rate": macro(ok, "impact_false_positive_rate"),
        "freshness_correct": sum(1 for row in ok if row.get("freshness_ok")) / len(ok)
        if ok
        else None,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gry", default="target/release/gry")
    parser.add_argument("--project", default="goca")
    parser.add_argument("--dataset", default="eval/tasks/goca-v1.json")
    parser.add_argument("--output", default="")
    parser.add_argument("--k", type=int, default=0)
    parser.add_argument("--max-tokens", type=int, default=0)
    parser.add_argument("--timeout", type=int, default=90)
    args = parser.parse_args()

    dataset: Suite = json.loads(Path(args.dataset).read_text())
    k = args.k or int(dataset.get("k", 10))
    if not args.max_tokens:
        args.max_tokens = int(dataset.get("max_tokens", 3200))
    rows = [evaluate(task, run_command(task, args), k) for task in dataset["tasks"]]
    dev = [row for row in rows if not row["holdout"]]
    holdout = [row for row in rows if row["holdout"]]
    report = {
        "dataset": {
            "version": dataset.get("version"),
            "path": args.dataset,
            "k": k,
            "max_tokens": args.max_tokens,
        },
        "project": args.project,
        "summary": {"development": summarize(dev), "holdout": summarize(holdout)},
        "tasks": rows,
    }
    text = json.dumps(report, indent=2, sort_keys=True)
    if args.output:
        Path(args.output).parent.mkdir(parents=True, exist_ok=True)
        Path(args.output).write_text(text + "\n")
        print(args.output)
    else:
        print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
