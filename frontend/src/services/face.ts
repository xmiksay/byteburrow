import { api } from '../utils/api'
import type {
  Contact,
  FaceRef,
  Page,
  RematchResult,
  CreateContactRequest,
  RenameContactRequest,
  AssignFaceRequest,
} from '../api'

/** Filters for `GET /api/face/refs`; `undefined` leaves a filter off. */
export interface FaceRefFilters {
  contact_id?: number
  confirmed?: boolean
  unassigned?: boolean
}

export const faceService = {
  async getContacts(): Promise<Contact[]> {
    return api.get<Contact[]>('/api/face/contacts')
  },

  async createContact(data: CreateContactRequest): Promise<Contact> {
    return api.post<Contact>('/api/face/contacts', data)
  },

  async renameContact(id: number, data: RenameContactRequest): Promise<Contact> {
    return api.put<Contact>(`/api/face/contacts/${id}`, data)
  },

  async deleteContact(id: number): Promise<{ message: string }> {
    return api.delete<{ message: string }>(`/api/face/contacts/${id}`)
  },

  async getRefs(
    filters: FaceRefFilters = {},
    page = 1,
    perPage = 50,
  ): Promise<Page<FaceRef>> {
    const query = new URLSearchParams()
    query.set('page', String(page))
    query.set('per_page', String(perPage))
    if (filters.contact_id !== undefined) {
      query.set('contact_id', String(filters.contact_id))
    }
    if (filters.confirmed !== undefined) {
      query.set('confirmed', String(filters.confirmed))
    }
    if (filters.unassigned !== undefined) {
      query.set('unassigned', String(filters.unassigned))
    }
    return api.get<Page<FaceRef>>(`/api/face/refs?${query.toString()}`)
  },

  async assignFace(id: number, data: AssignFaceRequest): Promise<FaceRef> {
    return api.put<FaceRef>(`/api/face/refs/${id}/assignment`, data)
  },

  async confirmFace(id: number, contactId?: number): Promise<{ message: string }> {
    // `contact_id` overrides any existing assignment in the same call; send it
    // only when the caller explicitly asked for one so the request body stays
    // `contact_id: null`-free (null would clear the label).
    const body = contactId !== undefined ? { contact_id: contactId } : {}
    return api.post<{ message: string }>(`/api/face/refs/${id}/confirm`, body)
  },

  async withdrawFace(id: number): Promise<{ message: string }> {
    return api.delete<{ message: string }>(`/api/face/refs/${id}/confirm`)
  },

  async rematch(): Promise<RematchResult> {
    return api.post<RematchResult>('/api/face/rematch')
  },
}
