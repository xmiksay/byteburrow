export CARGO_BUILD_JOBS ?= 4
export BYTEBURROW__PLUGIN_DIR ?= target/plugins

.DEFAULT_GOAL := help
.PHONY: help build run dev build-plugins frontend-install frontend-dev frontend-build frontend-dist-stub \
        check fmt fmt-check clippy lint frontend-typecheck frontend-lint \
        test test-unit test-integration frontend-test verify coverage install-hooks \
        openapi-spec openapi-generate openapi-check \
        migrate-up migrate-down clean

OPENAPI_SPEC ?= frontend/openapi.json

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'

## --- Build & run ---

build: build-plugins frontend-build ## Build everything for release (plugins + frontend + server)
	cargo build --release

run: build-plugins frontend-build ## Build and run everything in release mode
	cargo run --release --bin byteburrow

dev: build-plugins frontend-dist-stub ## Build plugins, run the server in debug mode (no frontend build)
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

frontend-dist-stub: ## Ensure frontend/dist exists (rust_embed needs the folder to compile) without a real build
	@mkdir -p frontend/dist
	@[ -e frontend/dist/index.html ] || echo '<!doctype html><title>ByteBurrow</title>' > frontend/dist/index.html

## --- OpenAPI / typed client ---

openapi-spec: frontend-dist-stub ## Dump the OpenAPI spec from the Rust code to frontend/openapi.json
	cargo run --quiet --bin byteburrow_cli -- openapi > $(OPENAPI_SPEC)
	@echo "Wrote $(OPENAPI_SPEC)"

openapi-generate: openapi-spec frontend-install ## Refresh the spec and regenerate the TS client types
	cd frontend && . "$$NVM_DIR/nvm.sh" && nvm use && npm run generate
	@echo "Regenerated frontend/src/api/schema.d.ts"

openapi-check: frontend-dist-stub ## Fail if the committed spec has drifted from the Rust code
	@cargo run --quiet --bin byteburrow_cli -- openapi > target/openapi-current.json
	@if ! diff -q $(OPENAPI_SPEC) target/openapi-current.json >/dev/null; then \
		echo "ERROR: $(OPENAPI_SPEC) is stale — run 'make openapi-generate' and commit the result."; \
		diff $(OPENAPI_SPEC) target/openapi-current.json | head -40; \
		exit 1; \
	fi
	@echo "OpenAPI spec is in sync with the Rust code."

## --- Quality gates ---

check: frontend-dist-stub ## Fast workspace typecheck (no codegen)
	cargo check --workspace

fmt: ## Apply rustfmt to the whole workspace
	cargo fmt --all

fmt-check: ## Check formatting without modifying files
	cargo fmt --all -- --check

clippy: frontend-dist-stub ## Lint the whole workspace, deny warnings
	cargo clippy --workspace --all-targets -- -D warnings

frontend-typecheck: frontend-install ## Type-check the frontend (vue-tsc --noEmit)
	cd frontend && . "$$NVM_DIR/nvm.sh" && nvm use && npm run typecheck

frontend-lint: frontend-install ## Lint the frontend (ESLint, flat config)
	cd frontend && . "$$NVM_DIR/nvm.sh" && nvm use && npm run lint

lint: fmt-check clippy openapi-check frontend-typecheck frontend-lint ## fmt-check + clippy + openapi drift check + frontend typecheck + frontend lint

## --- Tests ---

test-unit: frontend-dist-stub ## Unit tests (in-module #[cfg(test)])
	cargo test --workspace --lib --bins

test-integration: frontend-dist-stub ## Integration tests (tests/*.rs — needs DATABASE_URL, e.g. via docker-compose)
	@if ls tests/*.rs >/dev/null 2>&1; then \
		cargo test --workspace --test '*'; \
	else \
		echo "No integration tests yet (tests/ is empty)"; \
	fi

frontend-test: frontend-install ## Frontend unit tests (Vitest)
	cd frontend && . "$$NVM_DIR/nvm.sh" && nvm use && npm run test

test: test-unit test-integration frontend-test ## All tests

verify: lint test ## Pre-"done" gate: lint + all tests

coverage: frontend-dist-stub ## Generate an HTML coverage report (cargo-llvm-cov) into coverage/
	cargo llvm-cov --workspace --lib --bins --html --output-dir coverage
	@echo "Report: coverage/html/index.html"

install-hooks: ## Wire up the repo's git hooks (pre-push: run tests before pushing)
	git config core.hooksPath .githooks
	@echo "Git hooks installed (core.hooksPath=.githooks)"

## --- Database ---

migrate-up: ## Apply pending database migrations
	cargo run --bin byteburrow-migration up

migrate-down: ## Roll back the last database migration
	cargo run --bin byteburrow-migration down

## --- Cleanup ---

clean: ## Remove build artifacts (cargo + frontend dist)
	cargo clean
	rm -rf frontend/dist
