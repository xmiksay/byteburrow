export CARGO_BUILD_JOBS ?= 4
export BYTEBURROW__PLUGIN_DIR ?= target/plugins

.DEFAULT_GOAL := help
.PHONY: help build run dev build-plugins frontend-install frontend-dev frontend-build \
        check fmt fmt-check clippy lint frontend-typecheck \
        test test-unit test-integration verify \
        migrate-up migrate-down clean

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'

## --- Build & run ---

build: build-plugins frontend-build ## Build everything for release (plugins + frontend + server)
	cargo build --release

run: build-plugins frontend-build ## Build and run everything in release mode
	cargo run --release --bin byteburrow

dev: build-plugins ## Build plugins, run the server in debug mode (no frontend build)
	cargo run --bin byteburrow

build-plugins: ## Build all plugins as release cdylibs and symlink to target/plugins/
	@mkdir -p target/plugins
	@cargo build --release $$(for dir in plugins/*/; do \
		[ -f "$$dir/Cargo.toml" ] && echo "-p $$(grep '^name' "$$dir/Cargo.toml" | head -1 | sed 's/.*= *"\(.*\)"/\1/')"; \
	done)
	@for dir in plugins/*/; do \
		[ -f "$$dir/Cargo.toml" ] || continue; \
		name=$$(grep '^name' "$$dir/Cargo.toml" | head -1 | sed 's/.*= *"\(.*\)"/\1/' | tr '-' '_'); \
		so="target/release/lib$${name}.so"; \
		if [ -f "$$so" ]; then \
			ln -sf "../../$$so" "target/plugins/lib$${name}.so"; \
			echo "  -> target/plugins/lib$${name}.so"; \
		fi; \
	done
	@echo "Plugins ready in target/plugins"

frontend-install: ## Install frontend dependencies (nvm + npm)
	cd frontend && . "$$NVM_DIR/nvm.sh" && nvm use && npm install

frontend-dev: frontend-install ## Frontend dev server with hot reload
	cd frontend && . "$$NVM_DIR/nvm.sh" && nvm use && npm run dev

frontend-build: frontend-install ## Build the frontend for production
	cd frontend && . "$$NVM_DIR/nvm.sh" && nvm use && npm run build

## --- Quality gates ---

check: ## Fast workspace typecheck (no codegen)
	cargo check --workspace

fmt: ## Apply rustfmt to the whole workspace
	cargo fmt --all

fmt-check: ## Check formatting without modifying files
	cargo fmt --all -- --check

clippy: ## Lint the whole workspace, deny warnings
	cargo clippy --workspace --all-targets -- -D warnings

frontend-typecheck: frontend-install ## Type-check the frontend (vue-tsc --noEmit)
	cd frontend && . "$$NVM_DIR/nvm.sh" && nvm use && npm run typecheck

lint: fmt-check clippy frontend-typecheck ## fmt-check + clippy + frontend typecheck

## --- Tests ---
## NOTE: test-integration is currently a no-op — no tests/ directory exists yet
## (see docs/adr/0002-code-quality-remediation.md, item 4). Add integration
## tests under tests/ and this target will pick them up automatically.

test-unit: ## Unit tests (in-module #[cfg(test)])
	cargo test --workspace --lib --bins

test-integration: ## Integration tests (tests/ dir — currently empty)
	@if ls tests/*.rs >/dev/null 2>&1; then \
		cargo test --workspace --test '*'; \
	else \
		echo "No integration tests yet (tests/ is empty) — see docs/adr/0002-code-quality-remediation.md"; \
	fi

test: test-unit test-integration ## All tests

verify: lint test ## Pre-"done" gate: lint + all tests

## --- Database ---

migrate-up: ## Apply pending database migrations
	cargo run --bin byteburrow-migration up

migrate-down: ## Roll back the last database migration
	cargo run --bin byteburrow-migration down

## --- Cleanup ---

clean: ## Remove build artifacts (cargo + frontend dist)
	cargo clean
	rm -rf frontend/dist
