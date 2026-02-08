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
    },

    async createEntry(storageId: number, path: string, type: 'File' | 'Directory'): Promise<{ message: string }> {
        return api.post<{ message: string }>(`/api/storage/${storageId}/create/${path}`, { entry_type: type })
    },

    async renameEntry(storageId: number, oldPath: string, newPath: string): Promise<{ message: string }> {
        return api.post<{ message: string }>(`/api/storage/${storageId}/rename/${oldPath}`, { new_path: newPath })
    },

    async removeEntry(storageId: number, path: string): Promise<{ message: string }> {
        return api.delete<{ message: string }>(`/api/storage/${storageId}/remove/${path}`)
    },

    async updateEntryTags(storageId: number, path: string, tags: number[]): Promise<{ message: string }> {
        return api.put<{ message: string }>(`/api/storage/${storageId}/tags/${path}`, { tags })
    },

    async shareEntry(storageId: number, path: string, data: any): Promise<any> {
        return api.post<any>(`/api/storage/${storageId}/share/${path}`, data)
    },

    async getShares(storageId: number, path: string): Promise<any[]> {
        return api.get<any[]>(`/api/storage/${storageId}/share/${path}`)
    },

    async getSharesWithMe(): Promise<any[]> {
        return api.get<any[]>('/api/storage/share/with-me')
    },

    async listShareDirectory(shareId: number | string, path: string = ''): Promise<DirectoryListingResponse> {
        const encodedPath = path ? `/${path}` : ''
        return api.get<DirectoryListingResponse>(`/api/storage/share/${shareId}/list${encodedPath}?format=json`)
    },

    async getShareFileContent(shareId: number | string, path: string): Promise<string> {
        const token = localStorage.getItem('auth_token')
        const headers: Record<string, string> = {}
        if (token) {
            headers['Authorization'] = `Bearer ${token}`
        }

        const response = await fetch(`/api/storage/share/${shareId}/show/${path}`, {
            headers
        })
        if (!response.ok) throw new Error('Failed to fetch file content')
        return response.text()
    },

    async getShareInfo(shareId: number | string): Promise<any> {
        return api.get<any>(`/api/storage/share/${shareId}`)
    },

    async getAllShares(): Promise<any[]> {
        return api.get<any[]>('/api/storage/share')
    },

    async updateShare(shareId: number, data: any): Promise<any> {
        return api.put<any>(`/api/storage/share/${shareId}`, data)
    },

    async deleteShare(shareId: number): Promise<{ message: string }> {
        return api.delete<{ message: string }>(`/api/storage/share/${shareId}`)
    },

    // Share-based write operations
    async createShareEntry(shareId: number | string, path: string, entryType: 'Directory' | 'File'): Promise<{ message: string }> {
        return api.post<{ message: string }>(`/api/storage/share/${shareId}/create/${path}`, { entry_type: entryType })
    },

    async renameShareEntry(shareId: number | string, oldPath: string, newPath: string): Promise<{ message: string }> {
        return api.post<{ message: string }>(`/api/storage/share/${shareId}/rename/${oldPath}`, { new_path: newPath })
    },

    async removeShareEntry(shareId: number | string, path: string): Promise<{ message: string }> {
        return api.delete<{ message: string }>(`/api/storage/share/${shareId}/remove/${path}`)
    },

    async updateShareFileContent(shareId: number | string, path: string, content: string | Blob): Promise<{ message: string }> {
        const token = localStorage.getItem('auth_token')
        const headers: Record<string, string> = {}

        if (typeof content === 'string') {
            headers['Content-Type'] = 'text/plain'
        }

        if (token) {
            headers['Authorization'] = `Bearer ${token}`
        }

        const response = await fetch(`/api/storage/share/${shareId}/update/${path}`, {
            method: 'PUT',
            headers,
            body: content
        })
        if (!response.ok) {
            const err = await response.json().catch(() => ({ error: 'Failed to update file' }))
            throw new Error(err.error || 'Failed to update file')
        }
        return response.json()
    }
}
