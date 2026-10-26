# gry — development Makefile
#
# Common targets: make help

CARGO       ?= cargo
PROFILE     ?= release
BIN         ?= gry
PREFIX      ?= $(HOME)/.local
GRY_BIN     := target/$(PROFILE)/$(BIN)
CARGO_FLAGS ?=
TEST        ?=
PKG_NAME    := gry
PKG_VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)

.DEFAULT_GOAL := help

.PHONY: help all build build-debug release \
	check fmt fmt-check clippy lint test test-unit test-integration test-doc \
	doc doc-open run install uninstall bench differential smoke verify \
	package publish-dry-run publish version clean clean-target dist dist-clean

## help: list available targets
help:
	@echo "gry $(PKG_VERSION) — available targets:"
	@echo ""
	@awk 'BEGIN {FS = ":.*## "} /^## / { sub(/^## /, ""); split($$0, a, ": "); printf "  \033[36m%-18s\033[0m %s\n", a[1], a[2] }' $(MAKEFILE_LIST)
	@echo ""
	@echo "Variables: PROFILE=$(PROFILE) PREFIX=$(PREFIX) TEST='<filter>' CARGO_FLAGS='<flags>'"

## all: build the release binary
all: build

## build: build the release binary (target/release/gry)
build: release

## build-debug: build a debug binary (target/debug/gry)
build-debug:
	$(CARGO) build --workspace $(CARGO_FLAGS)

## release: build the release binary
release:
	$(CARGO) build --release --locked --bin $(BIN) $(CARGO_FLAGS)

## check: fast type-check of the whole workspace
check:
	$(CARGO) check --workspace --all-targets --locked $(CARGO_FLAGS)

## fmt: format the workspace
fmt:
	$(CARGO) fmt --all

## fmt-check: verify formatting without changing files
fmt-check:
	$(CARGO) fmt --all -- --check

## clippy: run clippy on all targets
clippy:
	$(CARGO) clippy --workspace --all-targets --locked -- -D warnings

## lint: formatting and clippy checks
lint: fmt-check clippy

## test: run the workspace test suite (TEST='name' to filter)
test:
	$(CARGO) test --workspace --locked $(if $(TEST),$(TEST),) $(CARGO_FLAGS)

## test-unit: run unit tests only
test-unit:
	$(CARGO) test --workspace --lib --locked $(if $(TEST),$(TEST),) $(CARGO_FLAGS)

## test-integration: run integration test binaries only
test-integration:
	$(CARGO) test --workspace --tests --locked $(if $(TEST),$(TEST),) $(CARGO_FLAGS)

## test-doc: run documentation tests
test-doc:
	$(CARGO) test --workspace --doc --locked $(CARGO_FLAGS)

## doc: build the API documentation (no dependency docs)
doc:
	$(CARGO) doc --workspace --no-deps --locked $(CARGO_FLAGS)

## doc-open: build and open the API documentation
doc-open:
	$(CARGO) doc --workspace --no-deps --locked --open $(CARGO_FLAGS)

## run: run the CLI (ARGS='doctor' to pass arguments)
run:
	$(CARGO) run --quiet --bin $(BIN) -- $(ARGS)

## install: install gry into PREFIX/bin (default ~/.local/bin)
install: release
	install -d "$(DESTDIR)$(PREFIX)/bin"
	install -m 0755 "$(GRY_BIN)" "$(DESTDIR)$(PREFIX)/bin/$(BIN)"
	@echo "installed $(DESTDIR)$(PREFIX)/bin/$(BIN)"

## uninstall: remove the installed binary from PREFIX/bin
uninstall:
	rm -f "$(DESTDIR)$(PREFIX)/bin/$(BIN)"

## smoke: index a fixture in a throwaway cache and check doctor/index/search
smoke: release
	@tmp="$$(mktemp -d "$${TMPDIR:-/tmp}/gry-smoke.XXXXXX")"; \
	trap 'rm -rf "$$tmp"' EXIT; \
	export GRY_CACHE_DIR="$$tmp/cache"; \
	echo "cache=$$GRY_CACHE_DIR"; \
	"$(GRY_BIN)" doctor --json >/dev/null; \
	"$(GRY_BIN)" index tests/fixtures --name smoke >/dev/null; \
	"$(GRY_BIN)" status smoke --diagnostics summary >/dev/null; \
	"$(GRY_BIN)" search smoke "" --limit 1 >/dev/null; \
	echo "smoke: PASS"

## bench: run scripts/benchmark.sh against REPO (REPO=/path/to/repo)
bench: release
	@test -n "$(REPO)" || { echo "usage: make bench REPO=/path/to/repo [NAME=project]" >&2; exit 2; }
	GRY_BIN="$(abspath $(GRY_BIN))" scripts/benchmark.sh "$(REPO)" $(if $(NAME),$(NAME),)

## differential: compare against the optional codebase-memory-mcp reference binary
differential: release
	GRY_BIN="$(abspath $(GRY_BIN))" tests/differential.sh

## verify: full pre-release verification checklist
verify: fmt-check test build smoke
	@echo "verify: PASS"

## package: check that every crate packages cleanly for crates.io
package:
	$(CARGO) package --workspace --locked --no-verify

## publish-dry-run: validate packaging for crates.io without uploading
publish-dry-run: verify
	DRY_RUN=1 bash scripts/publish.sh

## publish: publish every crate to crates.io in dependency order (CONFIRM=yes)
publish: verify package
	@test "$(CONFIRM)" = "yes" || { echo "refusing to publish without CONFIRM=yes" >&2; exit 2; }
	bash scripts/publish.sh

## version: print the workspace version and built binary version
version:
	@echo "workspace $(PKG_VERSION)"
	@if [ -x "$(GRY_BIN)" ]; then "$(GRY_BIN)" --version; else echo "$(GRY_BIN): not built"; fi

## dist: build a stripped release binary and print its checksum
dist: release
	@mkdir -p dist
	@cp "$(GRY_BIN)" "dist/$(BIN)"
	@if command -v shasum >/dev/null 2>&1; then shasum -a 256 "dist/$(BIN)" | tee "dist/$(BIN).sha256"; \
	else sha256sum "dist/$(BIN)" | tee "dist/$(BIN).sha256"; fi

## clean: remove build artifacts and the dist directory
clean: clean-target dist-clean

## clean-target: remove Cargo build artifacts
clean-target:
	$(CARGO) clean

## dist-clean: remove generated distribution files
dist-clean:
	rm -rf dist
