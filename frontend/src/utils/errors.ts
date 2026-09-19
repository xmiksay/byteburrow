/**
 * Extract a human-readable message from an unknown thrown value.
 *
 * `apiCall` throws `Error` carrying the server's message, but narrowing here
 * lets callers use `catch (err)` (implicit `unknown`) without `any` or a cast
 * at every call site.
 */
export function errorMessage(err: unknown): string {
  if (err instanceof Error) return err.message
  return String(err)
}
