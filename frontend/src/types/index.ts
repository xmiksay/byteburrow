// Application types are generated from the server's OpenAPI spec and re-exported
// from `src/api`. This barrel keeps the historical `@/types` import path working
// while guaranteeing the shapes stay in lock-step with the backend — see
// `src/api/index.ts`. Do not add hand-written model interfaces here.
export * from '../api'
