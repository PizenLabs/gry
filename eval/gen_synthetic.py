#!/usr/bin/env python3
"""Generate deterministic synthetic Go repositories for GRY benchmarks.

These fixtures preserve structural properties the algorithms exercise (fan-out,
depth, hubs, disconnected components) without pretending to be real projects.
They are labelled synthetic in every benchmark report.
"""

from __future__ import annotations

import argparse
import shutil
from pathlib import Path


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def medium(root: Path, files: int) -> None:
    write(root / "go.mod", "module example.test/medium\n\ngo 1.22\n")
    for i in range(files):
        pkg = f"pkg{i // 50}"
        calls = []
        for j in range(4):
            prev = f"pkg{(i - 1) // 50}.F{i - 1}_0()" if i else "1"
            calls.append(
                f"func F{i}_{j}() int {{ if {j} == 0 {{ return hub() + {prev} }}; return F{i}_0() + {j} }}"
            )
        write(
            root / pkg / f"f{i}.go",
            f"package {pkg}\n\n" + "\n".join(calls) + "\n",
        )
    write(root / "pkg0" / "hub.go", "package pkg0\n\nfunc hub() int { return 1 }\n")


def large(root: Path, files: int) -> None:
    write(root / "go.mod", "module example.test/large\n\ngo 1.22\n")
    for i in range(files):
        pkg = f"pkg{i // 100}"
        body = "\n".join(
            f"func F{i}_{j}() int {{ return {j} + hub() }}" for j in range(3)
        )
        write(root / pkg / f"f{i}.go", f"package {pkg}\n\n{body}\n")
    write(root / "pkg0" / "hub.go", "package pkg0\n\nfunc hub() int { return 1 }\n")


def hub(root: Path, callers: int) -> None:
    write(root / "go.mod", "module example.test/hub\n\ngo 1.22\n")
    write(root / "core" / "hub.go", "package core\n\nfunc Hub() int { return 1 }\n")
    for i in range(callers):
        pkg = f"callers{i // 100}"
        write(
            root / pkg / f"c{i}.go",
            f'package {pkg}\n\nfunc C{i}() int {{ return core.Hub() }}\n',
        )


def deep(root: Path, links: int) -> None:
    write(root / "go.mod", "module example.test/deep\n\ngo 1.22\n")
    for i in range(links):
        nxt = f"F{i + 1}()" if i + 1 < links else "0"
        write(
            root / "chain" / f"f{i}.go",
            f"package chain\n\nfunc F{i}() int {{ return {nxt} }}\n",
        )


def dense(root: Path, nodes: int, fanout: int) -> None:
    write(root / "go.mod", "module example.test/dense\n\ngo 1.22\n")
    funcs = []
    for i in range(nodes):
        calls = " + ".join(f"D{(i + j + 1) % nodes}()" for j in range(fanout))
        funcs.append(f"func D{i}() int {{ return {calls} }}")
    write(root / "dense" / "dense.go", "package dense\n\n" + "\n".join(funcs) + "\n")


def disconnected(root: Path, components: int, size: int) -> None:
    write(root / "go.mod", "module example.test/disconnected\n\ngo 1.22\n")
    for c in range(components):
        for i in range(size):
            nxt = f"G{c}_{i + 1}()" if i + 1 < size else "0"
            write(
                root / "parts" / f"c{c}" / f"g{i}.go",
                f"package c{c}\n\nfunc G{c}_{i}() int {{ return {nxt} }}\n",
            )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("kind", choices=["medium", "large", "hub", "deep", "dense", "disconnected"])
    parser.add_argument("root")
    parser.add_argument("--files", type=int, default=2000)
    parser.add_argument("--fanout", type=int, default=20)
    parser.add_argument("--components", type=int, default=100)
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args()
    root = Path(args.root)
    if root.exists():
        if not args.force:
            raise SystemExit(f"{root} exists; pass --force to replace")
        shutil.rmtree(root)
    root.mkdir(parents=True, exist_ok=True)
    if args.kind == "medium":
        medium(root, args.files)
    elif args.kind == "large":
        large(root, args.files)
    elif args.kind == "hub":
        hub(root, args.files)
    elif args.kind == "deep":
        deep(root, args.files)
    elif args.kind == "dense":
        dense(root, args.files, args.fanout)
    else:
        disconnected(root, args.components, args.files)
    print(root)


if __name__ == "__main__":
    main()
