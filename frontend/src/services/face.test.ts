import { describe, it, expect, afterEach, vi } from 'vitest'
import { faceService } from './face'

const jsonResponse = (body: unknown, ok = true, status = 200): Response =>
  ({ ok, status, json: () => Promise.resolve(body) }) as Response

describe('faceService', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('builds the refs query from the filters', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      jsonResponse({ items: [], page: 1, per_page: 50, total: 0, total_pages: 1 }),
    )
    vi.stubGlobal('fetch', fetchMock)

    await faceService.getRefs(
      { contact_id: 7, confirmed: true, unassigned: true },
      2,
      25,
    )

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/face/refs?page=2&per_page=25&contact_id=7&confirmed=true&unassigned=true',
      expect.objectContaining({ method: 'GET' }),
    )
  })

  it('omits unset filters from the refs query', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      jsonResponse({ items: [], page: 1, per_page: 50, total: 0, total_pages: 1 }),
    )
    vi.stubGlobal('fetch', fetchMock)

    await faceService.getRefs()

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/face/refs?page=1&per_page=50',
      expect.objectContaining({ method: 'GET' }),
    )
  })

  it('sends contact_id only when confirming with an explicit contact', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ message: 'ok' }))
    vi.stubGlobal('fetch', fetchMock)

    await faceService.confirmFace(3)
    await faceService.confirmFace(4, 9)

    const [first, second] = fetchMock.mock.calls
    expect(first[0]).toBe('/api/face/refs/3/confirm')
    expect(JSON.parse(first[1].body)).toEqual({})
    expect(second[0]).toBe('/api/face/refs/4/confirm')
    expect(JSON.parse(second[1].body)).toEqual({ contact_id: 9 })
  })

  it('clears a label by assigning null', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ id: 1 }))
    vi.stubGlobal('fetch', fetchMock)

    await faceService.assignFace(12, { contact_id: null })

    const [call] = fetchMock.mock.calls
    expect(call[0]).toBe('/api/face/refs/12/assignment')
    expect(call[1].method).toBe('PUT')
    expect(JSON.parse(call[1].body)).toEqual({ contact_id: null })
  })
})
