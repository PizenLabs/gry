## Summary

<!-- What does this change and why? Link the issue it resolves. -->

Fixes #

## Type of change

- [ ] Bug fix (non-breaking change that fixes an issue)
- [ ] New feature (non-breaking change that adds capability)
- [ ] Breaking change (fix or feature that changes existing behavior)
- [ ] Documentation or CI only
- [ ] Refactor or performance improvement

## Approach

<!-- Describe the implementation and any important tradeoffs. -->

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --locked`
- [ ] `cargo build --release --locked --bin gry`
- [ ] `./target/release/gry doctor --json`
- [ ] Added or updated tests for the change
- [ ] Updated documentation for user-visible behavior ([README](README.md), [CAPABILITIES](docs/CAPABILITIES.md), [BENCHMARKS](docs/BENCHMARKS.md), [CHANGELOG](CHANGELOG.md))
- [ ] Commits are signed off under the Developer Certificate of Origin (`git commit -s`)

## Notes for reviewers

<!-- Anything that needs special attention, follow-up work, or known limitations. -->
