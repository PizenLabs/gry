# GRY retrieval-quality report — 2026-11-03

## 1. Dataset and ground truth

The evaluation uses the reviewed fixture `eval/tasks/goca-v1.json` (revision `goca-eval-v1`), which
was written from the Go source and its tests, not generated from GRY output. It contains 9 tasks:

- 7 development tasks and 2 holdout tasks;
- request tracing, interface implementation, caller/callee discovery, test discovery, change impact,
  configuration/route navigation and documentation evidence;
- expected files, symbols (matched by QN suffix), relationships, tests, impact files, forbidden
  files, freshness and status;
- explicit rationale and normalization rules.

The fixture repository is `go-backend-clean-architecture`. It is read-only for the evaluation.
`K=15`, `max_output_tokens=6000`. Metrics are reported separately: file precision/recall/MRR, symbol
precision/recall/MRR, relationship recall, impact recall/false-positive rate, freshness correctness
and provenance coverage.

The runner invokes the public CLI (`gry context` / `gry hook`), not internal Rust functions.

## 2. Method

Both binaries were built in release mode from the same source revision and run against the same
fixture with isolated caches:

- **Baseline:** isolated worktree at `01e422f`.
- **Current:** working tree after remediation.

The fixture had one untracked file (`OHGO_EVALUATION_REPORT.md`) at measurement time, so it indexed
as 58 files / 446 nodes instead of the 57 files / 422 nodes recorded on 2026-10-27. This is fixture
drift, not a code difference; baseline and current index the same tree.

## 3. Results

### Development (7 tasks)

| Metric | Value |
|---|---:|
| Errors | 0 |
| File precision@15 | 0.484 |
| File recall@15 | 0.971 |
| File MRR | 0.857 |
| Symbol precision@15 | 0.316 |
| Symbol recall@15 | 0.881 |
| Symbol MRR | 0.625 |
| Relationship recall | 0.952 |
| Impact recall | 0.800 |
| Impact false-positive rate | 0.268 |
| Freshness correct | 7/7 |
| Provenance labelled | 1.000 |
| Verified relationship share | 0.868 |

### Holdout (2 tasks)

| Metric | Value |
|---|---:|
| Errors | 0 |
| File precision@15 | 0.236 |
| File recall@15 | 1.000 |
| File MRR | 1.000 |
| Symbol precision@15 | 0.225 |
| Symbol recall@15 | 1.000 |
| Symbol MRR | 1.000 |
| Relationship recall | 1.000 |
| Freshness correct | 2/2 |
| Provenance labelled | 1.000 |
| Verified relationship share | 0.923 |

Baseline and current produced **identical** summary metrics on this fixture. The published
2026-10-27 baseline (file precision 0.494, symbol recall 0.952, relation recall 1.000) was measured
before the extra untracked file existed; the difference is explained by fixture drift, not by the
remediation.

## 4. Interpretation

- **Recall is the strength.** The reviewed evidence is found: file and symbol recall are high, and
  the development holdout shows the method generalizes to unseen tasks.
- **Precision is structurally lower.** `get_task_context` intentionally returns supporting files
  (imports, tests, mocks, adjacent evidence). The expected set is a minimal answer set, not an
  exhaustive relevance judgment. Precision should not be read as “two thirds of the result is
  wrong.”
- **Relationship evidence is strong.** 95–100% recall with every returned relationship carrying a
  provenance label; 87–92% are `verified`, the remainder explicitly `inferred`, `declared` or
  `observed`.
- **Freshness is correct** in all 9 tasks. `complete_for_scope` is not over-claimed: the T01
  three-hop chain marks output truncation at 3200/6000 tokens and exposes `budget.omitted`.
- **Ambiguity is explicit.** The evaluation does not reward silently choosing a candidate; the
  context payload exposes unresolved targets and candidates.

## 5. False positives and false negatives

Known false negatives:

1. **Go type-only impact.** `domain.Task` struct/interface declarations are not graph nodes, so a
   change to `domain/task.go` does not reach `repository/task_repository.go` through the type
   annotation. T06 misses that file.
2. **Budget truncation.** At 3200 tokens the POST /task route → handler → use case → repository
   chain is trimmed; 6000 tokens is needed for the full chain. This is explicit, not silent.
3. **Bounded repository scan.** Freshness covers referenced paths and adjacent directories, not the
   whole repository.

Known false-positive sources:

- Supporting files that are relevant to the task but outside the minimal expected set.
- Inferred interface dispatch and infrastructure links (provenance is labelled).
- Directive/agent-state files and type-only references can add noise; discovery excludes common
  agent-state directories.

## 6. Ranking limitations

- Semantic vectors are static token embeddings, not a transformer encoder. They help lexical gaps
  but do not provide deep semantic matching.
- Hybrid ranking combines BM25 and graph proximity; weights were not re-tuned on this small fixture
  (avoiding overfitting).
- The context profile favors recall and evidence completeness. A stricter precision profile would
  need a separately reviewed dataset.

## 7. Reproduce

```sh
cargo build --release --locked --bin gry
export GRY_CACHE_DIR=/tmp/gry-eval-cache
./target/release/gry index /path/to/go-backend-clean-architecture --name goca --json
python3 eval/run_context_eval.py \
  --gry ./target/release/gry --project goca \
  --output /tmp/gry-eval.json
```

The output file contains per-task rows, commands, returned files/symbols, missing expectations,
forbidden files, provenance counts, budget truncation and freshness. The summary above is the
`summary` object in that file.

## 8. Remaining improvements

1. Add Go struct/interface type nodes and type-reference edges, then add a domain-type impact task.
2. Add a precision-oriented context profile, evaluated on a separately labelled set.
3. Add C/C++, Rust, Python, TypeScript and Java fixtures with reviewed ground truth; today the
   independent semantic fixture is one Go repository.
4. Measure P99 with 200+ samples per task.
5. Keep the fixture revision pinned and record untracked files before future comparisons.
