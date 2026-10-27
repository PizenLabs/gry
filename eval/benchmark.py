#!/usr/bin/env python3
"""Reproducible GRY context/hook benchmarks on real and synthetic graphs.

Real repositories and generated fixtures are reported separately. Latency is
the external CLI wall time (process startup included). Peak RSS is measured
with /usr/bin/time -l on macOS and /usr/bin/time -v on Linux.

The synthetic fixtures preserve graph shapes (fan-out, depth, hubs, dense and
disconnected components); they are not claims about real projects.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import re
import statistics
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import gen_synthetic  # noqa: E402


def sh(cmd: list[str], env: dict[str, str], timeout: int = 1800) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, capture_output=True, text=True, env=env, timeout=timeout)


def timed(cmd: list[str], env: dict[str, str]) -> tuple[subprocess.CompletedProcess, float, int]:
    """Run a command under /usr/bin/time and return its result, wall ms, RSS bytes."""
    wrapper = ["/usr/bin/time", "-l"] if platform.system() == "Darwin" else ["/usr/bin/time", "-v"]
    started = time.perf_counter()
    proc = sh(wrapper + cmd, env)
    elapsed = (time.perf_counter() - started) * 1000
    rss = 0
    if platform.system() == "Darwin":
        match = re.search(r"(\d+)\s+maximum resident set size", proc.stderr)
        if match:
            rss = int(match.group(1))
    else:
        match = re.search(r"Maximum resident set size \(kbytes\):\s*(\d+)", proc.stderr)
        if match:
            rss = int(match.group(1)) * 1024
    return proc, elapsed, rss


def q(values: list[float], fraction: float) -> float:
    values = sorted(values)
    return values[min(len(values) - 1, int(len(values) * fraction))]


def project_env(work: Path, project: str) -> dict[str, str]:
    env = os.environ.copy()
    env["GRY_CACHE_DIR"] = str(work / "caches" / project)
    return env


def index_dataset(gry: str, work: Path, project: str, root: Path, kind: str = "synthetic") -> dict:
    env = project_env(work, project)
    cmd = [gry, "index", str(root), "--name", project, "--json"]
    proc, wall_ms, rss = timed(cmd, env)
    if proc.returncode != 0:
        raise RuntimeError(f"index {project}: {proc.stderr[-1000:]}")
    report = json.loads(proc.stdout)
    return {
        "project": project,
        "kind": kind,
        "files": report.get("files"),
        "nodes": report.get("nodes"),
        "edges": report.get("edges"),
        "index_duration_ms": report.get("duration_ms"),
        "index_wall_ms": round(wall_ms, 1),
        "index_peak_rss_mb": round(rss / 1e6, 1),
        "unchanged": report.get("unchanged"),
        "parsed_files": report.get("parsed_files"),
        "reused_files": report.get("reused_files"),
        "deleted_files": report.get("deleted_files"),
        "root": str(root),
    }


def resolve_target(gry: str, project: str, env: dict[str, str], seed: str, fallback: str) -> str:
    proc = sh([gry, "search", project, seed, "--limit", "10", "--json"], env)
    if proc.returncode == 0:
        for row in json.loads(proc.stdout).get("results", []):
            if row.get("label") in ("Function", "Method", "Route"):
                return row["qn"]
    return fallback


def context_case(gry: str, project: str, env: dict[str, str], target: str, iterations: int,
                 max_tokens: int, depth: int) -> dict:
    cmd = [
        gry, "context", project, "--target", target, "--depth", str(depth),
        "--max-tokens", str(max_tokens), "--json",
    ]
    latencies: list[float] = []
    payloads: list[dict] = []
    for _ in range(iterations):
        started = time.perf_counter()
        proc = sh(cmd, env)
        latencies.append((time.perf_counter() - started) * 1000)
        if proc.returncode == 0:
            payloads.append(json.loads(proc.stdout))
    memories = []
    for _ in range(min(3, iterations)):
        _, _, rss = timed(cmd, env)
        if rss:
            memories.append(rss)
    statuses = sorted({row.get("status", "") for row in payloads})
    tokens = [row.get("budget", {}).get("estimated_tokens", 0) for row in payloads]
    sizes = [len(json.dumps(row)) for row in payloads]
    return {
        "target": target,
        "depth": depth,
        "max_tokens": max_tokens,
        "samples": iterations,
        "p50_ms": round(q(latencies, 0.50), 1),
        "p95_ms": round(q(latencies, 0.95), 1),
        "mean_ms": round(statistics.fmean(latencies), 1),
        "peak_rss_mb": round(max(memories) / 1e6, 1) if memories else None,
        "estimated_tokens": round(statistics.fmean(tokens), 1),
        "output_bytes": round(statistics.fmean(sizes), 1),
        "statuses": statuses,
        "truncated": sum(1 for row in payloads if row.get("budget", {}).get("truncated")),
        "max_nodes": max((len(row.get("nodes", [])) for row in payloads), default=0),
    }


def hook_case(gry: str, project: str, env: dict[str, str], stage: str, extra: list[str],
              iterations: int, cached: bool) -> dict:
    cmd = [gry, "hook", stage, "--project", project, "--json", "--max-tokens", "6000"] + extra
    if not cached:
        cmd.append("--no-cache")
    latencies, payloads = [], []
    for _ in range(iterations):
        started = time.perf_counter()
        proc = sh(cmd, env)
        latencies.append((time.perf_counter() - started) * 1000)
        if proc.returncode == 0:
            payloads.append(json.loads(proc.stdout))
    return {
        "stage": stage,
        "cached": cached,
        "samples": iterations,
        "p50_ms": round(q(latencies, 0.5), 1),
        "p95_ms": round(q(latencies, 0.95), 1),
        "statuses": sorted({row.get("hook", {}).get("status", "") for row in payloads}),
        "cached_hits": sum(1 for row in payloads if row.get("hook", {}).get("cached")),
        "estimated_tokens": round(
            statistics.fmean(
                row.get("context", {}).get("budget", {}).get("estimated_tokens", 0)
                for row in payloads
                if row.get("context")
            ),
            1,
        ) if payloads else None,
    }


def incremental_case(gry: str, project: str, env: dict[str, str], root: Path, work: Path) -> list[dict]:
    out = []
    repo = root
    out.append({**index_dataset(gry, work, project, repo), "operation": "unchanged"})
    files = sorted(repo.rglob("*.go"))
    if files:
        target = files[0]
        with target.open("a") as handle:
            handle.write("\n// benchmark incremental edit\n")
        out.append({**index_dataset(gry, work, project, repo), "operation": "edit-one"})
    added = repo / "bench_added.go"
    added.write_text("package bench\n\nfunc BenchAdded() int { return 0 }\n")
    out.append({**index_dataset(gry, work, project, repo), "operation": "add-one"})
    added.unlink()
    out.append({**index_dataset(gry, work, project, repo), "operation": "delete-one"})
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gry", default="target/release/gry")
    parser.add_argument("--workdir", required=True)
    parser.add_argument("--iterations", type=int, default=20)
    parser.add_argument("--large-files", type=int, default=20000)
    parser.add_argument("--skip-synthetic", action="store_true")
    parser.add_argument("--real-only", action="store_true")
    parser.add_argument("--goca", default="/Users/anvndev/Documents/Project/Benchmarks/go-backend-clean-architecture")
    parser.add_argument("--gry-repo", default=str(Path.cwd()))
    parser.add_argument("--output", default="")
    args = parser.parse_args()

    work = Path(args.workdir)
    work.mkdir(parents=True, exist_ok=True)
    gry = str(Path(args.gry).resolve())
    report: dict = {
        "environment": {
            "platform": platform.platform(),
            "python": platform.python_version(),
            "cpus": os.cpu_count(),
        },
        "datasets": [],
        "contexts": [],
        "hooks": [],
        "incremental": [],
    }

    datasets: list[tuple[str, Path]] = [
        ("goca", Path(args.goca)),
        ("gry", Path(args.gry_repo)),
    ]
    if not args.skip_synthetic and not args.real_only:
        synthetic = {
            "syn-medium": ("medium", 2000, 0, 0),
            "syn-large": ("large", args.large_files, 0, 0),
            "syn-hub": ("hub", 1500, 0, 0),
            "syn-deep": ("deep", 1500, 0, 0),
            "syn-dense": ("dense", 150, 20, 0),
            "syn-disconnected": ("disconnected", 8, 0, 100),
        }
        for project, (kind, files, fanout, components) in synthetic.items():
            root = work / project
            if not root.exists():
                root.mkdir(parents=True)
                if kind == "medium":
                    gen_synthetic.medium(root, files)
                elif kind == "large":
                    gen_synthetic.large(root, files)
                elif kind == "hub":
                    gen_synthetic.hub(root, files)
                elif kind == "deep":
                    gen_synthetic.deep(root, files)
                elif kind == "dense":
                    gen_synthetic.dense(root, files, fanout)
                else:
                    gen_synthetic.disconnected(root, components, files)
            datasets.append((project, root))

    for project, root in datasets:
        try:
            report["datasets"].append(index_dataset(gry, work, project, root, "real" if project in ("goca", "gry") else "synthetic"))
        except Exception as exc:  # keep the rest of the suite running
            report["datasets"].append({"project": project, "error": str(exc)})

    targets = {
        "goca": ("TaskController.Create", "goca.api.controller.task_controller.TaskController.Create"),
        "gry": ("get_task_context", "gry.crates.gry-query.src.context.get_task_context"),
        "syn-medium": ("F0_0", "syn-medium.pkg0.f0.F0_0"),
        "syn-large": ("F0_0", "syn-large.pkg0.f0.F0_0"),
        "syn-hub": ("C0", "syn-hub.callers0.c0.C0"),
        "syn-deep": ("F0", "syn-deep.chain.f0.F0"),
        "syn-dense": ("D0", "syn-dense.dense.dense.D0"),
        "syn-disconnected": ("G0_0", "syn-disconnected.parts.c0.g0.G0_0"),
    }
    for project, (seed, fallback) in targets.items():
        if not any(d["project"] == project and "error" not in d for d in report["datasets"]):
            continue
        env = project_env(work, project)
        target = resolve_target(gry, project, env, seed, fallback)
        iterations = args.iterations if project != "syn-large" else max(10, args.iterations // 2)
        report["contexts"].append(
            {**context_case(gry, project, env, target, iterations, 3200, 2), "project": project}
        )

    env = project_env(work, "goca")
    for stage, extra in [
        ("before_plan", ["--task", "Trace the POST /task request to the repository"]),
        ("before_edit", ["--target", "goca.api.controller.task_controller.TaskController.Create"]),
        ("after_edit", ["--changed-path", "usecase/task_usecase.go"]),
        ("before_finish", ["--changed-path", "usecase/task_usecase.go"]),
    ]:
        for cached in (False, True):
            report["hooks"].append(hook_case(gry, "goca", env, stage, extra, 10, cached))

    if not args.skip_synthetic and not args.real_only:
        for project in ("syn-medium", "syn-large"):
            if any(d["project"] == project and "error" in d for d in report["datasets"]):
                continue
            report["incremental"].append(
                {"project": project, "runs": incremental_case(gry, project, project_env(work, project), work / project, work)}
            )

    text = json.dumps(report, indent=2, sort_keys=True)
    if args.output:
        Path(args.output).parent.mkdir(parents=True, exist_ok=True)
        Path(args.output).write_text(text + "\n")
    print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
