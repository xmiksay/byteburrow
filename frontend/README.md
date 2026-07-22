# ByteBurrow Frontend (Vue 3 + TypeScript + Vite)

Vue 3 `<script setup>` SFCs with TypeScript, built by Vite. Use `nvm use` (see `.nvmrc`) before running raw `npm` commands.

## Scripts

| Command | Description |
|---|---|
| `npm run dev` | Vite dev server with hot reload (proxies `/api` to `localhost:3000`) |
| `npm run generate` | Regenerate `src/api/schema.d.ts` from `openapi.json` (`openapi-typescript`) |
| `npm run build` | Generate the API types, type-check (`vue-tsc`) and build into `dist/` |
| `npm run preview` | Serve the production build locally |
| `npm run typecheck` | Type-check only (`vue-tsc --noEmit`) |
| `npm run lint` / `npm run lint:fix` | ESLint (flat config, Vue 3 + TS) |
| `npm run test` / `npm run test:watch` | Vitest unit/component tests |

From the repo root these are also wrapped as `make frontend-dev`, `make frontend-build`, `make frontend-typecheck`, `make frontend-lint`, and `make frontend-test` (all handle `nvm use` automatically).

## API client (generated from OpenAPI)

Request/response types are **generated from the server's OpenAPI spec** — they are
never hand-written, so the frontend can't drift from the backend:

- `openapi.json` is the committed spec, dumped from the Rust code via `make openapi-spec`.
- `src/api/schema.d.ts` is generated from it by `openapi-typescript` (`npm run generate`,
  also run automatically by `npm run build`). It is a generated file — don't edit it.
- `src/api/index.ts` re-exports friendly aliases (`User`, `Storage`, `Shared`, request
  bodies, …). Import model types from `@/types` (which re-exports `src/api`) and call
  endpoints through `src/services/*` + the `api` helper in `src/utils/api.ts`.

After changing an API type on the server, run `make openapi-generate` from the repo root
and commit both `frontend/openapi.json` and `frontend/src/api/schema.d.ts`. `make lint`
runs `openapi-check`, which fails if the committed spec is stale.

## Testing

Tests use [Vitest](https://vitest.dev/) with a `jsdom` environment and [`@vue/test-utils`](https://test-utils.vuejs.org/) for component mounting. Test files live next to the code they cover as `*.test.ts` (e.g. `src/utils/file.test.ts`, `src/composables/useAuth.test.ts`, `src/components/HelloWorld.test.ts`).

## Linting

ESLint uses the flat config (`eslint.config.js`) with `eslint-plugin-vue` and `@vue/eslint-config-typescript`. Some pre-existing patterns (`no-explicit-any` at the API boundary, prop mutation in the media/file viewers) are currently demoted to warnings so they surface as cleanup targets without blocking CI.
