# Contributing to gry

Thank you for your interest in gry. Contributions of all kinds are welcome:
bug reports, feature proposals, documentation fixes, tests, benchmarks, and
code.

By participating in this project, you agree to follow our
[Code of Conduct](CODE_OF_CONDUCT.md).

## Ways to contribute

- **Report a bug.** Open an issue with a minimal reproduction, expected
  behavior, and the output of `gry --version` and `gry doctor --json`.
- **Propose a feature.** Describe the problem and the workflow it unlocks.
  Check [docs/CAPABILITIES.md](docs/CAPABILITIES.md) first so we can keep the
  documented scope coherent.
- **Improve documentation.** Documentation fixes are reviewed like code.
- **Contribute code.** Start with an issue for non-trivial changes so we can
  agree on the approach before you invest in a large patch.
- **Report a vulnerability.** Follow [SECURITY.md](SECURITY.md). Never open a
  public issue for a security problem.

## Before you start

- Search existing issues and pull requests to avoid duplicate work.
- Keep changes focused. A pull request should solve one problem.
- For large features, language additions, schema changes, or public API changes,
  open an issue first and wait for maintainer feedback.

## Development setup

### Requirements

- Rust 1.85+ (edition 2021)
- A C toolchain for the statically linked tree-sitter grammars and bundled
  SQLite
- Git, for change-impact and history-coupling features
- Optional: `codebase-memory-mcp` (`cbm`) on `PATH` for the differential check

### Build and test

```bash
git clone https://github.com/PizenLabs/gry.git
cd gry
cargo build --release
cargo test --workspace
./target/release/gry --version
```

The workspace layout, test strategy, and release process are documented in
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

### Run the full verification set

```bash
make verify
```

Or run the steps individually:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --release --locked --bin gry
./target/release/gry doctor --json
```

`make smoke` indexes a fixture in a throwaway cache and exercises doctor,
index, status, and search. `make differential` performs the optional comparison
with the reference implementation when it is installed.

## Making a change

1. Fork the repository and create a topic branch:

   ```bash
   git switch -c feat/short-description
   ```

2. Make the smallest coherent change. Follow the style of the surrounding code.
3. Add or update tests for every behavior change and every fixed defect.
4. Update documentation when behavior is user-visible:
   - [README.md](README.md) for install, CLI, MCP, or configuration changes;
   - [docs/CAPABILITIES.md](docs/CAPABILITIES.md) for coverage or limits;
   - [docs/BENCHMARKS.md](docs/BENCHMARKS.md) for measured performance;
   - [THIRD_PARTY.md](THIRD_PARTY.md) for new dependencies or grammar crates;
   - [CHANGELOG.md](CHANGELOG.md) under `Unreleased`.
5. Run the verification set above.
6. Commit with a clear message and open a pull request.

### Commit messages

Use [Conventional Commits](https://www.conventionalcommits.org/) with the
imperative mood:

```text
feat(index): add Kotlin companion-object symbols
fix(mcp): bound pagination cursor under cancellation
docs: clarify GRY_ALLOWED_ROOT behavior
test(query): cover unresolved call evidence
```

Common types are `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`,
`ci`, and `chore`. Keep each commit reviewable on its own.

### Developer Certificate of Origin

Contributions are accepted under the Apache License 2.0. By submitting a pull
request, you certify that you have the right to contribute the work under that
license. Sign off your commits to record this:

```bash
git commit -s -m "feat: ..."
```

The sign-off adds a `Signed-off-by:` trailer using your Git identity. See the
[Developer Certificate of Origin 1.1](https://developercertificate.org/) for the
full statement.

## Pull requests

A good pull request:

- links the issue or describes the motivation;
- explains the approach and any important tradeoffs;
- includes tests and documentation updates;
- keeps unrelated refactors out of the diff;
- passes CI on supported platforms.

CI runs formatting checks, the workspace test suite, a release build, and
`gry doctor`. Maintainers may ask for changes before merging. Review is
best-effort; if a pull request goes quiet, a polite ping is welcome.

## Coding guidelines

- Use `rustfmt`; CI rejects unformatted code.
- Treat `cargo clippy -- -D warnings` as the target for new code.
- Prefer explicit errors over panics in library and server paths.
- Preserve graph evidence semantics: exact, inferred, ambiguous, and unresolved
  must remain distinguishable.
- Do not introduce runtime network access, telemetry, or a required daemon.
- Keep measured claims in [docs/BENCHMARKS.md](docs/BENCHMARKS.md); do not copy
  numbers into ad-hoc documentation.
- Add tests that fail before the fix and pass after it.

### Adding a language grammar

Language support touches several places. In general:

1. Add the grammar crate to `crates/gry-parse/Cargo.toml`.
2. Register the language and node mappings in `crates/gry-parse/src/registry.rs`.
3. Add extraction fixtures and tests for definitions, calls, and imports.
4. Add the crate and license to [THIRD_PARTY.md](THIRD_PARTY.md).
5. Update the supported-language list in [README.md](README.md) and
   [docs/CAPABILITIES.md](docs/CAPABILITIES.md).

If a grammar is not available or not compatible, use the conservative lexical
fallback and mark the limitation in the capability matrix.

## Reporting bugs

Include:

- the gry version and install method;
- operating system and architecture;
- the exact command or MCP tool invocation;
- a minimal reproduction repository or fixture;
- expected and actual behavior;
- relevant logs, with `RUST_LOG=gry=debug` when useful;
- `gry doctor --json` output.

Remove secrets, proprietary source, and personal paths before posting.

## Feature requests

Explain the user problem first, then the proposed behavior. Include the kind of
repository and language mix where it matters. Requests that fit the documented
scope are easier to schedule; see the scope section in
[docs/CAPABILITIES.md](docs/CAPABILITIES.md).

## Documentation style

- Write for an engineer who has not used gry before.
- Prefer short paragraphs, tables, and runnable examples.
- Link to the canonical document instead of duplicating details.
- Keep terminology consistent: "node", "edge", "qualified name", "generation",
  "delta index", "MCP tool".

## License

By contributing, you agree that your contributions are licensed under the
[Apache License, Version 2.0](LICENSE), and you certify the contribution under
the Developer Certificate of Origin as described above.
