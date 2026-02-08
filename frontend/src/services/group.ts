import { api } from '../utils/api'
import type { Group, CreateGroupRequest, UpdateGroupRequest } from '../types'

export const groupService = {
    async getGroups(): Promise<Group[]> {
        return api.get<Group[]>('/api/group')
    },

    async createGroup(data: CreateGroupRequest): Promise<Group> {
        return api.post<Group>('/api/group', data)
    },

    async updateGroup(id: number, data: UpdateGroupRequest): Promise<Group> {
        return api.put<Group>(`/api/group/${id}`, data)
    },

    async deleteGroup(id: number): Promise<{ message: string }> {
        return api.delete<{ message: string }>(`/api/group/${id}`)
    }
}
