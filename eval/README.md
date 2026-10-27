# GRY context evaluation

This directory contains the independent, manually reviewed semantic evaluation
for `gry context` and `gry hook`, plus the reproducible performance and synthetic
graph harnesses.

The evaluation target is
`/Users/anvndev/Documents/Project/Benchmarks/go-backend-clean-architecture`
(module `github.com/amitshekhariitbhu/go-backend-clean-architecture`). The fixture
is used **read-only**. GRY and OHGO must not become runtime dependencies of it.

## Dataset

`tasks/goca-v1.json` contains 9 tasks: 7 development tasks and 2 holdout tasks.
Each task declares:

- a stable id and category;
- the real interface (`context` or `hook`) and arguments;
- expected project-relative files;
- expected symbols, matched by QN suffix so a project prefix is not ground truth;
- expected relationships (`source`, `type`, `target`);
- expected tests and impact files where applicable;
- expected status and freshness;
- files that must not appear;
- a short reviewed rationale.

The expectations were written from the Go source and its tests. They were not
generated from GRY output. When GRY and an expectation disagreed, the mismatch
was reviewed first; expectations changed only when the reviewed source showed
the expectation was wrong.

### Normalization and denominators

- Files are exact project-relative paths with `./` removed.
- Symbols are compared by exact QN or QN suffix (`suffix`, `.suffix`). Returned
  QNs are deduplicated; duplicate entities never inflate precision or recall.
- Expected relations are present when a returned relationship matches source,
  target and one of the declared types (for example `TESTS` or `CALLS`).
- `K=15` by default. Expected sets are the denominator; missing expectations are
  never silently filled from GRY output.
- Impact recall uses reviewed `impact_files`. Impact false-positive rate uses
  inbound callers/importers/handlers reported by the context. A changed file
  itself is not counted as impact evidence.

## Metrics

Reported separately, never merged into one opaque score:

- file precision@K, recall@K and MRR;
- symbol precision@K, recall@K and MRR;
- relationship recall;
- impact recall and impact false-positive rate;
- freshness correctness (status plus `evidence.freshness`), and whether
  `complete_for_scope` is claimed;
- output tokens/bytes, truncation and the engine's own status.

## Run

From the GRY repository root:

```sh
cargo build --release --locked --bin gry
export GRY_CACHE_DIR=/tmp/gry-eval-cache
./target/release/gry index \
  /Users/anvndev/Documents/Project/Benchmarks/go-backend-clean-architecture \
  --name goca --json
python3 eval/run_context_eval.py \
  --gry ./target/release/gry --project goca \
  --output /tmp/eval.json
```

The runner invokes the public CLI only. Hook tasks use `gry hook ... --no-cache`
so repeated runs are comparable and do not read a previous payload.

## Baseline (2026-10-27, generation `1793042092015-6930c932c8af42cd948732e997096090`)

Dataset revision `goca-eval-v1`, K=15, `max_output_tokens=6000`.

| Split | Tasks | File P@K | File R@K | File MRR | Symbol P@K | Symbol R@K | Symbol MRR | Relation recall | Impact recall | Impact FP rate | Freshness |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Development | 7 | 0.494 | 1.000 | 0.929 | 0.333 | 0.952 | 0.640 | 1.000 | 0.800 | 0.268 | 7/7 |
| Holdout | 2 | 0.236 | 1.000 | 1.000 | 0.225 | 1.000 | 1.000 | 1.000 | — | — | 2/2 |

Evidence quality: every returned relationship carried a known provenance label
(1.0 development and holdout). The share of `verified` relationships was 0.866
on development and 0.923 on holdout; the remainder were explicitly `inferred`
(ambiguous interface dispatch), `declared` (manifest/infra) or `observed`.

Interpretation:

- The reviewed evidence is found (recall 1.0) and the first relevant file/symbol
  is usually early (MRR ≥ 0.64). Precision is lower because a bounded context
  intentionally returns supporting files (imports, usage, tests, mocks) that are
  relevant to the task but not in the minimal expected set. Precision is a
  strict metric here, not a claim that every extra row is wrong.
- `T06` misses `repository/task_repository.go` for a `domain.Task` change and
  reports one unsupported caller (`api/route/route.go`). Go struct and
  interface type declarations are not graph nodes, so impact through a type
  annotation is not fully modeled. This is a known limitation, not hidden by
  changing the fixture.
- `T01` needs `max_output_tokens=6000` to keep the full three-hop
  route → handler → use case → repository chain. At the CLI default of 3200,
  the hard budget trims the chain (the target and one neighbour survive). This
  is the expected trade-off; callers can raise the budget.

### Budget behavior

At 3200 tokens a three-hop Go request trace is truncated. The payload remains
valid JSON, `budget.truncated=true`, `budget.omitted` names the removed arrays,
`evidence.complete_for_scope=false` and the reason `output_truncated` is added.
At 6000 tokens the full chain above fits.

## Performance harness

`benchmark.py` generates deterministic synthetic repositories with
`gen_synthetic.py`, indexes them into isolated caches, and measures the external
CLI. It reports synthetic and real repositories separately.

```sh
python3 eval/benchmark.py \
  --gry ./target/release/gry \
  --workdir /tmp/gry-bench \
  --iterations 20 \
  --output eval/results/2026-10-27-context-hooks.json
```

Results in `results/2026-10-27-context-hooks.json`:

| Dataset | Kind | Files | Nodes | Edges | Index ms | Index peak RSS |
|---|---|---:|---:|---:|---:|---:|
| goca | real | 57 | 422 | 958 | 707 | 105.7 MB |
| gry | real | 95 | 3,817 | 17,898 | 1,177 | 192.4 MB |
| medium | synthetic | 2,002 | 10,047 | 48,393 | 1,514 | 327.9 MB |
| large | synthetic | 20,002 | 80,207 | 160,207 | 8,703 | 1,327.9 MB |
| hub (1,500 callers) | synthetic | 1,502 | 3,023 | 6,023 | 965 | 136.4 MB |
| deep (1,500 chain) | synthetic | 1,501 | 3,006 | 6,004 | 995 | 145.8 MB |
| dense (150×20) | synthetic | 2 | 157 | 3,503 | 705 | 107.2 MB |
| disconnected (100×8) | synthetic | 801 | 1,706 | 3,205 | 866 | 122.4 MB |

| Context target | Samples | p50 | p95 | Peak RSS | Tokens |
|---|---:|---:|---:|---:|---:|
| goca | 20 | 10.2 ms | 11.2 ms | 12.1 MB | 2,858 |
| gry | 20 | 13.3 ms | 20.6 ms | 14.7 MB | 2,854 |
| medium 2k | 20 | 23.1 ms | 25.9 ms | 29.8 MB | 2,822 |
| large 20k | 10 | 80.6 ms | 84.9 ms | 107.8 MB | 2,883 |
| hub | 20 | 15.2 ms | 16.4 ms | 16.7 MB | 2,932 |
| deep | 20 | 12.3 ms | 13.7 ms | 12.7 MB | 917 |
| dense | 20 | 13.7 ms | 14.3 ms | 13.6 MB | 2,919 |
| disconnected | 20 | 9.6 ms | 10.5 ms | 11.8 MB | 974 |

Hooks on goca (10 samples each):

| Stage | No-cache p50/p95 | Cached p50/p95 |
|---|---:|---:|
| before_plan | 15.3 / 16.6 ms | 9.6 / 16.1 ms |
| before_edit | 10.2 / 11.2 ms | 9.6 / 11.0 ms |
| after_edit | 14.3 / 14.6 ms | 9.8 / 14.7 ms |
| before_finish | 14.3 / 16.2 ms | 10.1 / 14.5 ms |

Incremental indexing (synthetic):

| Dataset | Operation | Parsed | Deleted | ms | Peak RSS |
|---|---|---:|---:|---:|---:|
| medium | unchanged | 0 | 0 | 206 | 21.5 MB |
| medium | edit one | 1 | 0 | 102 | 25.1 MB |
| medium | add one | 1 | 0 | 749 | 125.4 MB |
| medium | delete one | 0 | 1 | 141 | 44.1 MB |
| large | unchanged | 0 | 0 | 789 | 58.8 MB |
| large | edit one | 1 | 0 | 712 | 64.0 MB |
| large | add one | 1 | 0 | 1,481 | 289.0 MB |
| large | delete one | 0 | 1 | 941 | 219.6 MB |

All context measurements respect the 3200-token hard budget and return valid
JSON. The large graph has 10 samples: p95 is reported, p99 is **not** claimed.

## Limitations

- 100,000 files were not tested. The 20,002-file result is the measured ceiling.
- Synthetic graph shapes are structurally realistic but not real repositories.
- The evaluation covers one independent Go repository; it is not a universal
  quality threshold.
- Go struct/interface type nodes are not extracted, so type-only impact paths
  (for example `domain.Task` → repository) are under-detected.
- P99 values need more samples. The harness reports p50/p95 and the sample count.
- `gry hook` process startup is ~6.6 ms (`gry --version`); persistent-process
  optimization is not justified by the measured 9.6–15.3 ms hook latency.
- The bounded adjacent-directory scan does not re-apply `.gitignore`/`.gryignore`;
  a new ignored source file beside referenced evidence may be reported as an
  added file. That is conservative (stale, not complete) but it can be noise.
