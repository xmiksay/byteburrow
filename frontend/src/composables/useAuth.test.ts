import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest'
import { useAuth } from './useAuth'

const jsonResponse = (body: unknown, ok = true, status = 200): Response =>
  ({ ok, status, json: () => Promise.resolve(body) }) as Response

describe('useAuth', () => {
  beforeEach(async () => {
    // Reset the module-level singleton state between tests.
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({}, false, 401)))
    await useAuth().logout()
    vi.restoreAllMocks()
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('logs in and populates the user on success', async () => {
    const me = { id: 1, username: 'alice', name: 'Alice', admin: true }
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse({ expires_in_days: 30 })) // /login
      .mockResolvedValueOnce(jsonResponse(me)) // /me
    vi.stubGlobal('fetch', fetchMock)

    const auth = useAuth()
    const ok = await auth.login('alice', 'pw')

    expect(ok).toBe(true)
    expect(auth.isAuthenticated.value).toBe(true)
    expect(auth.user.value).toEqual(me)
    expect(auth.error.value).toBeNull()
  })

  it('surfaces the server error message on failed login', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(jsonResponse({ error: 'bad credentials' }, false, 401)),
    )

    const auth = useAuth()
    const ok = await auth.login('alice', 'wrong')

    expect(ok).toBe(false)
    expect(auth.isAuthenticated.value).toBe(false)
    expect(auth.error.value).toBe('bad credentials')
  })

  it('clears the user on logout', async () => {
    const me = { id: 1, username: 'alice', name: 'Alice', admin: false }
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValueOnce(jsonResponse({ expires_in_days: 30 }))
        .mockResolvedValueOnce(jsonResponse(me)),
    )
    const auth = useAuth()
    await auth.login('alice', 'pw')
    expect(auth.isAuthenticated.value).toBe(true)

    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({})))
    await auth.logout()
    expect(auth.isAuthenticated.value).toBe(false)
    expect(auth.user.value).toBeNull()
  })
})
