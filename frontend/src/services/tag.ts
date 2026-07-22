import { api } from '../utils/api'
import type { Tag, CreateTagRequest, UpdateTagRequest } from '../types'

export const tagService = {
    async getTags(): Promise<Tag[]> {
        return api.getAll<Tag>('/api/tag')
    },

    async createTag(data: CreateTagRequest): Promise<Tag> {
        return api.post<Tag>('/api/tag', data)
    },

    async updateTag(id: number, data: UpdateTagRequest): Promise<Tag> {
        return api.put<Tag>(`/api/tag/${id}`, data)
    },

    async deleteTag(id: number): Promise<{ message: string }> {
        return api.delete<{ message: string }>(`/api/tag/${id}`)
    }
}
