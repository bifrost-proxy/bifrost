.PHONY: all build build-release clean test lint fmt install-deps dev help setup build-cli

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
	cargo run -p bifrost-cli -- start

# Run the proxy server in release mode
run-release:
	cargo run -p bifrost-cli --release -- start

# Development mode with hot reload for frontend
dev:
	@echo "Starting frontend dev server..."
	cd web && npm run dev &
	@echo "Starting backend..."
	SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-cli -- start --verbose

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
