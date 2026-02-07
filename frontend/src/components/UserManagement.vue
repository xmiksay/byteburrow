<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { Users, Edit, Trash2, X, Save, UserPlus } from 'lucide-vue-next'
import { api } from '../utils/api'

export interface UserResponse {
  id: number
  name: string
  description?: string
  username: string
  enabled: boolean
  admin: boolean
}

interface CreateUserRequest {
  name: string
  description?: string
  username: string
  password: string
  enabled?: boolean
  admin?: boolean
}

interface UpdateUserRequest {
  name?: string
  description?: string
  username?: string
  password?: string
  enabled?: boolean
  admin?: boolean
}

const users = ref<UserResponse[]>([])
const loading = ref(true)
const error = ref<string | null>(null)

// Modal state
const showModal = ref(false)
const modalMode = ref<'create' | 'edit'>('create')
const editingUserId = ref<number | null>(null)

// Form state
const formData = ref({
  name: '',
  description: '',
  username: '',
  password: '',
  enabled: true,
  admin: false,
})

const formErrors = ref<Record<string, string>>({})
const submitting = ref(false)

const fetchUsers = async () => {
  try {
    loading.value = true
    error.value = null
    users.value = await api.get<UserResponse[]>('/api/user')
  } catch (err: any) {
    error.value = err.message
  } finally {
    loading.value = false
  }
}

const openCreateModal = () => {
  modalMode.value = 'create'
  editingUserId.value = null
  formData.value = {
    name: '',
    description: '',
    username: '',
    password: '',
    enabled: true,
    admin: false,
  }
  formErrors.value = {}
  showModal.value = true
}

const openEditModal = (user: UserResponse) => {
  modalMode.value = 'edit'
  editingUserId.value = user.id
  formData.value = {
    name: user.name,
    description: user.description || '',
    username: user.username,
    password: '', // Don't populate password for security
    enabled: user.enabled,
    admin: user.admin,
  }
  formErrors.value = {}
  showModal.value = true
}

const closeModal = () => {
  showModal.value = false
  editingUserId.value = null
  formErrors.value = {}
}

const validateForm = (): boolean => {
  formErrors.value = {}

  if (!formData.value.name.trim()) {
    formErrors.value.name = 'Name is required'
  }

  if (!formData.value.username.trim()) {
    formErrors.value.username = 'Username is required'
  }

  if (modalMode.value === 'create' && !formData.value.password) {
    formErrors.value.password = 'Password is required'
  }

  return Object.keys(formErrors.value).length === 0
}

const handleSubmit = async () => {
  if (!validateForm()) return

  try {
    submitting.value = true
    error.value = null

    if (modalMode.value === 'create') {
      const payload: CreateUserRequest = {
        name: formData.value.name,
        username: formData.value.username,
        password: formData.value.password,
        enabled: formData.value.enabled,
        admin: formData.value.admin,
      }
      if (formData.value.description) {
        payload.description = formData.value.description
      }
      await api.post<UserResponse>('/api/user', payload)
    } else {
      const payload: UpdateUserRequest = {
        name: formData.value.name,
        username: formData.value.username,
        enabled: formData.value.enabled,
        admin: formData.value.admin,
      }
      if (formData.value.description) {
        payload.description = formData.value.description
      }
      if (formData.value.password) {
        payload.password = formData.value.password
      }
      await api.put<UserResponse>(`/api/user/${editingUserId.value}`, payload)
    }

    await fetchUsers()
    closeModal()
  } catch (err: any) {
    error.value = err.message
  } finally {
    submitting.value = false
  }
}

const deleteUser = async (userId: number, username: string) => {
  if (!confirm(`Are you sure you want to delete user "${username}"?`)) {
    return
  }

  try {
    error.value = null
    await api.delete(`/api/user/${userId}`)
    await fetchUsers()
  } catch (err: any) {
    error.value = err.message
  }
}

onMounted(() => {
  fetchUsers()
})
</script>

<template>
  <div class="user-management">
    <div class="area-header">
      <div>
        <h2>User Management</h2>
        <p>Manage user accounts and permissions.</p>
      </div>
      <button class="btn-primary" @click="openCreateModal">
        <UserPlus :size="18" />
        <span>Create User</span>
      </button>
    </div>

    <div v-if="error" class="error-banner glass-panel">
      <p>{{ error }}</p>
      <button @click="error = null" class="btn-icon">
        <X :size="16" />
      </button>
    </div>

    <div v-if="loading" class="loading-state">
      <div class="spinner"></div>
      <p>Loading users...</p>
    </div>

    <div v-else class="users-table glass-panel">
      <table>
        <thead>
          <tr>
            <th>Name</th>
            <th>Username</th>
            <th>Description</th>
            <th>Status</th>
            <th>Role</th>
            <th>Actions</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="user in users" :key="user.id">
            <td>
              <div class="user-cell">
                <Users :size="16" />
                <span>{{ user.name }}</span>
              </div>
            </td>
            <td>
              <span class="username">{{ user.username }}</span>
            </td>
            <td>
              <span class="description">{{ user.description || '-' }}</span>
            </td>
            <td>
              <span :class="['status-badge', user.enabled ? 'enabled' : 'disabled']">
                {{ user.enabled ? 'Active' : 'Disabled' }}
              </span>
            </td>
            <td>
              <span :class="['role-badge', user.admin ? 'admin' : 'user']">
                {{ user.admin ? 'Admin' : 'User' }}
              </span>
            </td>
            <td>
              <div class="actions">
                <button class="btn-icon" @click="openEditModal(user)" title="Edit user">
                  <Edit :size="16" />
                </button>
                <button class="btn-icon danger" @click="deleteUser(user.id, user.username)" title="Delete user">
                  <Trash2 :size="16" />
                </button>
              </div>
            </td>
          </tr>
        </tbody>
      </table>

      <div v-if="users.length === 0" class="empty-state">
        <Users :size="48" style="opacity: 0.3" />
        <p>No users found.</p>
      </div>
    </div>

    <!-- Modal -->
    <div v-if="showModal" class="modal-overlay" @click.self="closeModal">
      <div class="modal glass-panel">
        <div class="modal-header">
          <h3>{{ modalMode === 'create' ? 'Create User' : 'Edit User' }}</h3>
          <button class="btn-icon" @click="closeModal">
            <X :size="20" />
          </button>
        </div>

        <form @submit.prevent="handleSubmit" class="modal-body">
          <div class="form-group">
            <label for="name">Name *</label>
            <input
              id="name"
              v-model="formData.name"
              type="text"
              placeholder="Full name"
              :class="{ error: formErrors.name }"
            />
            <span v-if="formErrors.name" class="error-message">{{ formErrors.name }}</span>
          </div>

          <div class="form-group">
            <label for="username">Username *</label>
            <input
              id="username"
              v-model="formData.username"
              type="text"
              placeholder="username"
              :class="{ error: formErrors.username }"
            />
            <span v-if="formErrors.username" class="error-message">{{ formErrors.username }}</span>
          </div>

          <div class="form-group">
            <label for="password">
              Password {{ modalMode === 'edit' ? '(leave empty to keep current)' : '*' }}
            </label>
            <input
              id="password"
              v-model="formData.password"
              type="password"
              placeholder="••••••••"
              :class="{ error: formErrors.password }"
            />
            <span v-if="formErrors.password" class="error-message">{{ formErrors.password }}</span>
          </div>

          <div class="form-group">
            <label for="description">Description</label>
            <textarea
              id="description"
              v-model="formData.description"
              placeholder="Optional description"
              rows="3"
            ></textarea>
          </div>

          <div class="form-row">
            <div class="form-group checkbox">
              <label>
                <input type="checkbox" v-model="formData.enabled" />
                <span>Account enabled</span>
              </label>
            </div>

            <div class="form-group checkbox">
              <label>
                <input type="checkbox" v-model="formData.admin" />
                <span>Administrator</span>
              </label>
            </div>
          </div>

          <div class="modal-footer">
            <button type="button" class="btn-secondary" @click="closeModal" :disabled="submitting">
              Cancel
            </button>
            <button type="submit" class="btn-primary" :disabled="submitting">
              <Save :size="16" />
              <span>{{ submitting ? 'Saving...' : 'Save' }}</span>
            </button>
          </div>
        </form>
      </div>
    </div>
  </div>
</template>

<style scoped>
.user-management {
  display: flex;
  flex-direction: column;
  gap: 24px;
}

.area-header {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
}

.area-header h2 {
  font-size: 1.75rem;
  margin-bottom: 4px;
}

.area-header p {
  color: var(--text-secondary);
}

.btn-primary {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 20px;
  background: var(--accent-color);
  border: none;
  border-radius: 8px;
  color: white;
  cursor: pointer;
  font-weight: 500;
  transition: all 0.2s;
}

.btn-primary:hover:not(:disabled) {
  background: rgba(59, 130, 246, 0.8);
  transform: translateY(-2px);
}

.btn-primary:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.error-banner {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 16px;
  background: rgba(239, 68, 68, 0.1);
  border: 1px solid rgba(239, 68, 68, 0.3);
  border-radius: 8px;
  color: #ef4444;
}

.users-table {
  padding: 0;
  overflow: hidden;
}

table {
  width: 100%;
  border-collapse: collapse;
}

thead {
  background: rgba(255, 255, 255, 0.02);
  border-bottom: 1px solid var(--border-color);
}

th {
  padding: 16px;
  text-align: left;
  font-weight: 600;
  color: var(--text-secondary);
  font-size: 0.875rem;
}

td {
  padding: 16px;
  border-bottom: 1px solid var(--border-color);
}

tbody tr:hover {
  background: rgba(255, 255, 255, 0.02);
}

tbody tr:last-child td {
  border-bottom: none;
}

.user-cell {
  display: flex;
  align-items: center;
  gap: 8px;
  font-weight: 500;
}

.username {
  font-family: monospace;
  color: var(--text-secondary);
}

.description {
  color: var(--text-secondary);
  font-size: 0.875rem;
}

.status-badge, .role-badge {
  font-size: 0.75rem;
  padding: 4px 10px;
  border-radius: 20px;
  font-weight: 500;
}

.status-badge.enabled {
  background: rgba(16, 185, 129, 0.1);
  color: var(--success-color);
  border: 1px solid rgba(16, 185, 129, 0.2);
}

.status-badge.disabled {
  background: rgba(107, 114, 128, 0.1);
  color: #9ca3af;
  border: 1px solid rgba(107, 114, 128, 0.2);
}

.role-badge.admin {
  background: rgba(139, 92, 246, 0.1);
  color: #a78bfa;
  border: 1px solid rgba(139, 92, 246, 0.2);
}

.role-badge.user {
  background: rgba(59, 130, 246, 0.1);
  color: var(--accent-color);
  border: 1px solid rgba(59, 130, 246, 0.2);
}

.actions {
  display: flex;
  gap: 8px;
}

.btn-icon {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 8px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  border-radius: 6px;
  color: var(--text-primary);
  cursor: pointer;
  transition: all 0.2s;
}

.btn-icon:hover {
  background: rgba(255, 255, 255, 0.08);
  border-color: var(--accent-color);
}

.btn-icon.danger:hover {
  background: rgba(239, 68, 68, 0.1);
  border-color: #ef4444;
  color: #ef4444;
}

.loading-state, .empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 80px;
  text-align: center;
  gap: 16px;
}

.spinner {
  width: 40px;
  height: 40px;
  border: 3px solid rgba(255, 255, 255, 0.1);
  border-top-color: var(--accent-color);
  border-radius: 50%;
  animation: spin 1s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}

/* Modal */
.modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.7);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
  padding: 20px;
}

.modal {
  width: 100%;
  max-width: 500px;
  max-height: 90vh;
  overflow-y: auto;
  animation: slideUp 0.2s ease-out;
}

@keyframes slideUp {
  from {
    opacity: 0;
    transform: translateY(20px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}

.modal-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 24px;
  border-bottom: 1px solid var(--border-color);
}

.modal-header h3 {
  font-size: 1.25rem;
}

.modal-body {
  padding: 24px;
  display: flex;
  flex-direction: column;
  gap: 20px;
}

.form-group {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.form-group label {
  font-size: 0.875rem;
  font-weight: 500;
  color: var(--text-secondary);
}

.form-group input,
.form-group textarea {
  padding: 10px 12px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  color: var(--text-primary);
  font-size: 0.875rem;
  transition: all 0.2s;
}

.form-group input:focus,
.form-group textarea:focus {
  outline: none;
  border-color: var(--accent-color);
  background: rgba(255, 255, 255, 0.08);
}

.form-group input.error {
  border-color: #ef4444;
}

.error-message {
  font-size: 0.75rem;
  color: #ef4444;
}

.form-group.checkbox label {
  flex-direction: row;
  align-items: center;
  gap: 8px;
  cursor: pointer;
}

.form-group.checkbox input[type="checkbox"] {
  width: auto;
  cursor: pointer;
}

.form-row {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 16px;
}

.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: 12px;
  padding-top: 16px;
  border-top: 1px solid var(--border-color);
}

.btn-secondary {
  padding: 10px 20px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  color: var(--text-primary);
  cursor: pointer;
  font-weight: 500;
  transition: all 0.2s;
}

.btn-secondary:hover:not(:disabled) {
  background: rgba(255, 255, 255, 0.08);
  border-color: var(--accent-color);
}

.btn-secondary:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
</style>
