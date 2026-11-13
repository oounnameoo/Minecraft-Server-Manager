.PHONY: dev build build-cli test test-deploy lint test-web verify

BUILD_COMMIT := $(shell git rev-parse HEAD 2>/dev/null || echo unknown)
BUILD_DIRTY := $(shell test -z "$$(git status --porcelain 2>/dev/null)" && echo false || echo true)
BUILD_TIME := $(shell date -u +%Y-%m-%dT%H:%M:%SZ)

# Build the Rust backend and CLI, embedding the frontend build.
build:
	cd web && pnpm build
	cd rust && \
		MCM_BUILD_COMMIT=$(BUILD_COMMIT) \
		MCM_BUILD_DIRTY=$(BUILD_DIRTY) \
		MCM_BUILD_TIME=$(BUILD_TIME) \
		cargo build --release --workspace
	cp rust/target/release/mcm ./mcm
	cp rust/target/release/mcm-cli ./mcm-cli

build-cli:
	cd rust && \
		MCM_BUILD_COMMIT=$(BUILD_COMMIT) \
		MCM_BUILD_DIRTY=$(BUILD_DIRTY) \
		MCM_BUILD_TIME=$(BUILD_TIME) \
		cargo build --release -p mcm-cli
	cp rust/target/release/mcm-cli ./mcm-cli

dev:
	cd rust && cargo run -p mcm-server

test:
	cd rust && cargo test --workspace

test-deploy:
	PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s deploy/tests -v

lint:
	cd rust && cargo clippy --workspace --all-targets

test-web:
	cd web && pnpm typecheck && pnpm lint && pnpm test && pnpm build

verify: build test test-deploy test-web
