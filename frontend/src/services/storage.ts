import { api } from '../utils/api'
import type { Storage, CreateStorageRequest, UpdateStorageRequest, DirectoryListingResponse } from '../types'

export const storageService = {
    async getStorages(): Promise<Storage[]> {
        return api.get<Storage[]>('/api/storage')
    },

    async createStorage(data: CreateStorageRequest): Promise<Storage> {
        return api.post<Storage>('/api/storage', data)
    },

    async updateStorage(id: number, data: UpdateStorageRequest): Promise<Storage> {
        return api.put<Storage>(`/api/storage/${id}`, data)
    },

    async deleteStorage(id: number): Promise<{ message: string }> {
        return api.delete<{ message: string }>(`/api/storage/${id}`)
    },

    async listDirectory(storageId: number, path: string = ''): Promise<DirectoryListingResponse> {
        const encodedPath = path ? `/${path}` : ''
        return api.get<DirectoryListingResponse>(`/api/storage/${storageId}/list${encodedPath}?format=json`)
    },

    async getFileContent(storageId: number, path: string): Promise<string> {
        const response = await fetch(`/api/storage/${storageId}/show/${path}`, {
            headers: {
                'Authorization': `Bearer ${localStorage.getItem('auth_token')}`
            }
        })
        if (!response.ok) throw new Error('Failed to fetch file content')
        return response.text()
    },

    async updateFileContent(storageId: number, path: string, content: string): Promise<{ message: string }> {
        return api.putRaw<{ message: string }>(`/api/storage/${storageId}/update/${path}`, content)
    }
}
