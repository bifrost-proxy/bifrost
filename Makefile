.PHONY: all build build-release clean test lint fmt install-deps dev help setup build-cli \
        coverage coverage-unit coverage-e2e coverage-html coverage-json coverage-crate \
	coverage-gate coverage-diff

# Development servers must never adopt the installed service's ~/.bifrost
# runtime/PID files or its production-like 9900 listener. Override these only
# when intentionally operating a disposable development instance.
BIFROST_DEV_DATA_DIR ?= $(CURDIR)/.bifrost-dev
BIFROST_DEV_PORT ?= 8800
BIFROST_DEV_ENV = BIFROST_DATA_DIR="$(BIFROST_DEV_DATA_DIR)" BIFROST_DISABLE_TRAY=1 BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
BIFROST_DEV_START_ARGS = -p $(BIFROST_DEV_PORT) --no-system-proxy --skip-cert-check

# Default target
all: build

# Build in debug mode (includes frontend build)
build:
	cargo build --workspace

# Build in release mode (optimized, includes frontend build)
build-release:
	cargo build --workspace --release

# Build CLI only (command-line version with web UI)
build-cli:
	cargo build -p bifrost-cli --release

# Build without frontend (for faster iteration on backend)
build-backend:
	SKIP_FRONTEND_BUILD=1 cargo build --workspace

# Build only the frontend
build-frontend:
	cd web && npm install && npm run build

# Run the proxy server in debug mode
run:
	$(BIFROST_DEV_ENV) cargo run -p bifrost-cli -- start $(BIFROST_DEV_START_ARGS)

# Run the proxy server in release mode
run-release:
	$(BIFROST_DEV_ENV) cargo run -p bifrost-cli --release -- start $(BIFROST_DEV_START_ARGS)

# Development mode with hot reload for frontend
dev:
	@echo "Starting frontend dev server..."
	cd web && npm run dev &
	@echo "Starting backend..."
	$(BIFROST_DEV_ENV) SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-cli -- start $(BIFROST_DEV_START_ARGS) --verbose

# Clean all build artifacts
clean:
	cargo clean
	rm -rf web/dist web/node_modules

# Run all tests
test:
	cargo test --workspace

# Run tests with verbose output
test-verbose:
	cargo test --workspace -- --nocapture

# Run linter
lint:
	cargo clippy --workspace -- -D warnings
	cd web && npm run lint 2>/dev/null || true

# Coverage targets ----------------------------------------------------------
# Unified unit + integration coverage gate. Fails when any crate drops below
# its ratcheted floor (goal: 90%) or the workspace aggregate floor.
coverage: coverage-gate

# Unit + integration coverage only (no E2E binaries built).
coverage-unit:
	bash scripts/ci/coverage.sh

# Coverage for the E2E suites (instrumented `bifrost` + `bifrost-e2e` binaries).
coverage-e2e:
	bash scripts/ci/coverage-e2e.sh

# Workspace-wide unified report, enforcing the per-crate ratcheted floors
# (scripts/ci/coverage-thresholds.toml; goal: 90%) plus the workspace floor.
coverage-gate:
	bash scripts/ci/coverage-all.sh --json --gate --gaps

# Changed production Rust lines against BASE_REF must be covered by the LCOV
# report produced by coverage-all.sh. Usage: make coverage-diff BASE_REF=origin/main
coverage-diff:
	@test -n "$(BASE_REF)" || { echo "Usage: make coverage-diff BASE_REF=<git-ref>" >&2; exit 2; }
	python3 scripts/ci/coverage-diff.py target/coverage/lcov.info --base-ref "$(BASE_REF)" --threshold 95

# HTML report (output: target/coverage/html/index.html). Skips the gate
# so a low-coverage report is still browsable.
coverage-html:
	bash scripts/ci/coverage-all.sh --html --fail-under 0

# JSON summary printed to stdout (used by CI to ingest coverage metrics).
coverage-json:
	bash scripts/ci/coverage-all.sh --json --fail-under 0

# Per-crate coverage helper. Usage: `make coverage-crate CRATE=bifrost-command`.
coverage-crate:
	@if [ -z "$(CRATE)" ]; then \
	  echo "Usage: make coverage-crate CRATE=<crate-name>" >&2; exit 2; \
	fi
	bash scripts/ci/coverage-crate.sh $(CRATE) --text --fail-under 90

# Format code
fmt:
	cargo fmt --all
	cargo fmt --manifest-path desktop/src-tauri/Cargo.toml --all
	cd web && npm run format 2>/dev/null || true

# Check formatting without making changes
fmt-check:
	cargo fmt --all -- --check
	cargo fmt --manifest-path desktop/src-tauri/Cargo.toml --all -- --check

# Install development dependencies
install-deps:
	cd web && npm install

# Setup development environment (install git hooks)
setup: setup-hooks

setup-hooks:
	@bash scripts/setup-git-hooks.sh

# Create release artifacts
release: build-release
	@echo "Release build complete!"
	@echo "Binary location: target/release/bifrost"
	@ls -lh target/release/bifrost 2>/dev/null || true

# Package for distribution (creates tarball)
package: build-release
	@mkdir -p dist
	@cp target/release/bifrost dist/
	@cd dist && tar -czvf bifrost-$(shell cargo pkgid -p bifrost-cli | cut -d# -f2).tar.gz bifrost
	@echo "Package created in dist/"
	@ls -lh dist/

# Show help
help:
	@echo "Bifrost Proxy Build System"
	@echo ""
	@echo "Usage: make [target]"
	@echo ""
	@echo "Targets:"
	@echo "  build          Build in debug mode (default)"
	@echo "  build-release  Build in release mode (optimized)"
	@echo "  build-cli      Build CLI version only (with web UI)"
	@echo "  build-backend  Build backend only (skip frontend)"
	@echo "  build-frontend Build frontend only"
	@echo "  run            Run proxy server in debug mode (CLI)"
	@echo "  run-release    Run proxy server in release mode (CLI)"
	@echo "  dev            Development mode with frontend hot reload"
	@echo "  clean          Clean all build artifacts"
	@echo "  test           Run all tests"
	@echo "  test-verbose   Run tests with verbose output"
	@echo "  coverage       Run unified unit + integration coverage gate (90% line gate)"
	@echo "  coverage-unit  Unit + integration coverage only"
	@echo "  coverage-e2e   E2E coverage (instrumented bifrost + bifrost-e2e)"
	@echo "  coverage-html  Generate HTML coverage report"
	@echo "  coverage-json  Generate JSON coverage summary"
	@echo "  coverage-crate Coverage for a single crate (CRATE=<name>)"
	@echo "  lint           Run linter on all code"
	@echo "  fmt            Format all code"
	@echo "  fmt-check      Check code formatting"
	@echo "  install-deps   Install development dependencies"
	@echo "  setup          Setup development environment (git hooks)"
	@echo "  release        Create release build"
	@echo "  package        Create distribution package"
	@echo "  help           Show this help message"
	@echo ""
	@echo "Environment variables:"
	@echo "  SKIP_FRONTEND_BUILD=1  Skip frontend build during cargo build"
