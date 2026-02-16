import { api } from '../utils/api'
import type { Photo } from '../types'

export const photoService = {
    async listAll(): Promise<Photo[]> {
        return api.get<Photo[]>('/api/photo')
    },

    async listByYear(year: number): Promise<Photo[]> {
        return api.get<Photo[]>(`/api/photo/list/${year}`)
    },

    async listByMonth(year: number, month: number): Promise<Photo[]> {
        return api.get<Photo[]>(`/api/photo/list/${year}/${month}`)
    },

    async listByDay(year: number, month: number, day: number): Promise<Photo[]> {
        return api.get<Photo[]>(`/api/photo/list/${year}/${month}/${day}`)
    },

    async regenerateThumbnail(hash: string): Promise<void> {
        await api.post(`/api/photo/regenerate/${hash}`)
    }
}
