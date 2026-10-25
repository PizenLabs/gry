# Third-Party Notices

gry is licensed under the Apache License 2.0 (see `LICENSE` and `NOTICE`). It depends on open-source
Rust crates and redistributes the Apache-2.0 Nomic token vectors. All direct Rust dependencies report
OSI-approved licenses; `cargo metadata` reports no `UNKNOWN` license among the 273 resolved packages.

## Nomic token vectors — Apache-2.0

`third_party/nomic/` contains:

- `code_tokens.txt` — filtered vocabulary (40,856 tokens)
- `code_vectors.bin` — 40,856 × 768 int8 unit vectors
- `LICENSE` — Apache License 2.0
- `NOTICE` — attribution

Source: <https://huggingface.co/nomic-ai/nomic-embed-code>, © Nomic AI. The vectors are derived from
the upstream model by running full inference over a filtered token vocabulary and quantizing to int8.
No other part of the model is redistributed. gry uses these static token embeddings; it does not
run the upstream 7B model.

## SQLite

`rusqlite` is MIT. The bundled SQLite library is public domain
(<https://www.sqlite.org/copyright.html>). FTS5 is part of SQLite.

## Tree-sitter and grammars

`tree-sitter` (MIT) and the statically linked grammar crates below. Each crate's own license is
reproduced in the Cargo registry source and applies to that crate. Unless marked otherwise the
grammar crates below are MIT; `tree-sitter-elixir`, `tree-sitter-hcl`, `tree-sitter-ini` are
Apache-2.0.

| Crate | Version | License |
|---|---|---|
| `cortexkit-tree-sitter-scss` | 1.0.1 | MIT |
| `tree-sitter` | 0.25.10 | MIT |
| `tree-sitter-ada` | 0.9.1 | MIT |
| `tree-sitter-bash` | 0.25.1 | MIT |
| `tree-sitter-c` | 0.24.2 | MIT |
| `tree-sitter-c-sharp` | 0.23.5 | MIT |
| `tree-sitter-cmake` | 0.7.5 | MIT |
| `tree-sitter-commonlisp` | 0.4.1 | MIT |
| `tree-sitter-containerfile` | 0.9.2 | MIT |
| `tree-sitter-cpp` | 0.23.4 | MIT |
| `tree-sitter-crystal` | 0.1.0 | MIT |
| `tree-sitter-css` | 0.25.0 | MIT |
| `tree-sitter-cuda` | 0.21.2 | MIT |
| `tree-sitter-d` | 0.8.2 | MIT |
| `tree-sitter-dart` | 0.3.0 | MIT |
| `tree-sitter-diff` | 0.1.0 | MIT |
| `tree-sitter-elixir` | 0.3.5 | Apache-2.0 |
| `tree-sitter-elm` | 5.9.4 | MIT |
| `tree-sitter-erlang` | 0.21.0 | MIT |
| `tree-sitter-fortran` | 0.6.0 | MIT |
| `tree-sitter-fsharp` | 0.3.12 | MIT |
| `tree-sitter-gdscript` | 6.1.0 | MIT |
| `tree-sitter-glsl` | 0.2.0 | MIT |
| `tree-sitter-go` | 0.25.0 | MIT |
| `tree-sitter-graphql` | 0.3.0 | MIT |
| `tree-sitter-groovy` | 0.1.2 | MIT |
| `tree-sitter-haskell` | 0.24.1 | MIT |
| `tree-sitter-hcl` | 1.1.0 | Apache-2.0 |
| `tree-sitter-html` | 0.23.2 | MIT |
| `tree-sitter-ini` | 1.4.0 | Apache-2.0 |
| `tree-sitter-java` | 0.23.5 | MIT |
| `tree-sitter-javascript` | 0.25.0 | MIT |
| `tree-sitter-jinja2` | 0.0.16 | MIT |
| `tree-sitter-json` | 0.24.8 | MIT |
| `tree-sitter-julia` | 0.23.1 | MIT |
| `tree-sitter-kt` | 0.1.0 | MIT |
| `tree-sitter-language` | 0.1.9 | MIT |
| `tree-sitter-lua` | 0.5.0 | MIT |
| `tree-sitter-luau` | 1.2.0 | MIT |
| `tree-sitter-make` | 1.1.1 | MIT |
| `tree-sitter-md-025` | 0.5.6 | MIT |
| `tree-sitter-nickel` | 0.5.0 | MIT |
| `tree-sitter-nix` | 0.3.0 | MIT |
| `tree-sitter-ocaml` | 0.26.0 | MIT |
| `tree-sitter-pascal` | 0.10.2 | MIT |
| `tree-sitter-php` | 0.25.1 | MIT |
| `tree-sitter-powershell` | 0.26.4 | MIT |
| `tree-sitter-prisma` | 0.2.2 | MIT |
| `tree-sitter-properties` | 0.3.0 | MIT |
| `tree-sitter-python` | 0.25.0 | MIT |
| `tree-sitter-qmljs` | 0.3.1 | MIT |
| `tree-sitter-r` | 1.3.0 | MIT |
| `tree-sitter-regex` | 0.25.0 | MIT |
| `tree-sitter-ruby` | 0.23.1 | MIT |
| `tree-sitter-rust` | 0.24.2 | MIT |
| `tree-sitter-scala` | 0.26.3 | MIT |
| `tree-sitter-scheme` | 0.25.0 | MIT |
| `tree-sitter-sequel` | 0.3.11 | MIT |
| `tree-sitter-solidity` | 1.2.13 | MIT |
| `tree-sitter-starlark` | 1.3.0 | MIT |
| `tree-sitter-svelte-ng` | 1.0.2 | MIT |
| `tree-sitter-swift` | 0.7.4 | MIT |
| `tree-sitter-toml-ng` | 0.7.0 | MIT |
| `tree-sitter-typescript` | 0.23.2 | MIT |
| `tree-sitter-verilog` | 1.0.3 | MIT |
| `tree-sitter-vhdl` | 1.4.0 | MIT |
| `tree-sitter-vue-next` | 0.1.0 | MIT |
| `tree-sitter-yaml` | 0.7.2 | MIT |
| `tree-sitter-zig` | 1.1.2 | MIT |

## Rust dependencies (direct)

`serde`, `serde_json`, `thiserror`, `anyhow`, `regex`, `globset`, `ignore`, `walkdir`, `sha2`, `hex`,
`rayon`, `crossbeam-channel`, `parking_lot`, `tracing`, `tracing-subscriber`, `clap`, `zstd`,
`once_cell`, `uuid`, `chrono`, `notify`, `tempfile`, `xxhash-rust`, `itertools`, `memchr`, `fs2`,
`base64`, `serde_yaml`, `toml`, `rusqlite`, `rmp-serde`, `pdf-extract` (and its `lopdf`/`pom`/
`ttf-parser` dependencies). All are MIT or MIT/Apache-2.0; `cargo metadata` reports no `UNKNOWN`
license among the 273 resolved packages (see `Cargo.lock` and each crate's metadata).


