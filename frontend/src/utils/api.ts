import type { components, Page } from '../api'

/** Error envelope returned by the API (`ErrorResponse` in the OpenAPI spec). */
export type ApiError = components['schemas']['ErrorResponse']

export type { Page }

export async function apiCall<T>(
  endpoint: string,
  options: RequestInit = {}
): Promise<T> {
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    ...(options.headers as Record<string, string>),
  }

  const response = await fetch(endpoint, {
    ...options,
    headers,
    credentials: 'same-origin',
  })

  if (!response.ok) {
    const errorData: ApiError = await response.json().catch(() => ({
      error: `Request failed with status ${response.status}`,
    }))
    throw new Error(errorData.error)
  }

  return response.json()
}

export const api = {
  get: <T>(endpoint: string) => apiCall<T>(endpoint, { method: 'GET' }),

  post: <T>(endpoint: string, data?: any) =>
    apiCall<T>(endpoint, {
      method: 'POST',
      body: data ? JSON.stringify(data) : undefined,
    }),

  put: <T>(endpoint: string, data?: any) =>
    apiCall<T>(endpoint, {
      method: 'PUT',
      body: data ? JSON.stringify(data) : undefined,
    }),

  putRaw: <T>(endpoint: string, data: string) =>
    apiCall<T>(endpoint, {
      method: 'PUT',
      body: data,
      headers: {
        'Content-Type': 'text/plain',
      },
    }),

  delete: <T>(endpoint: string) =>
    apiCall<T>(endpoint, { method: 'DELETE' }),

  /** GET a single page of a paginated list endpoint. */
  getPage: <T>(endpoint: string, page = 1, perPage = 200) => {
    const sep = endpoint.includes('?') ? '&' : '?'
    return apiCall<Page<T>>(`${endpoint}${sep}page=${page}&per_page=${perPage}`, {
      method: 'GET',
    })
  },

  /**
   * GET every page of a paginated list endpoint and return the flattened
   * items. Lets callers keep a plain `T[]` view while the API is paginated.
   */
  getAll: async <T>(endpoint: string, perPage = 200): Promise<T[]> => {
    const first = await api.getPage<T>(endpoint, 1, perPage)
    const items = [...first.items]
    for (let page = 2; page <= first.total_pages; page++) {
      const next = await api.getPage<T>(endpoint, page, perPage)
      items.push(...next.items)
    }
    return items
  },
}
