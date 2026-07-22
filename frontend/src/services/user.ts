import { api } from '../utils/api'
import type { User } from '../types'

export const userService = {
    async getUsers(): Promise<User[]> {
        return api.getAll<User>('/api/user')
    },

    async changePassword(userId: number, password: string): Promise<{ message: string }> {
        return api.post<{ message: string }>(`/api/user/${userId}/password`, { password })
    }
}
