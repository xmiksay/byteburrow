import { api } from '../utils/api'
import type { User } from '../types'

export const userService = {
    async getUsers(): Promise<User[]> {
        return api.get<User[]>('/api/user')
    }
}
