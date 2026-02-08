<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { Database, Edit, Trash2, X, Save, Plus } from 'lucide-vue-next'
import { storageService } from '../services/storage'
import { userService } from '../services/user'
import { groupService } from '../services/group'
import UserSelect from './UserSelect.vue'
import GroupSelect from './GroupSelect.vue'
import type { Storage, User, Group, CreateStorageRequest, UpdateStorageRequest } from '../types'

const storages = ref<Storage[]>([])
const users = ref<User[]>([])
const groups = ref<Group[]>([])
const loading = ref(true)
const error = ref<string | null>(null)

// Modal state
const showModal = ref(false)
const modalMode = ref<'create' | 'edit'>('create')
const editingStorageId = ref<number | null>(null)

// Form state
const formData = ref({
  name: '',
  description: '',
  path: '',
  default_user: 1,
  default_group: 1,
})

const formErrors = ref<Record<string, string>>({})
const submitting = ref(false)

const fetchData = async () => {
  try {
    loading.value = true
    error.value = null
    const [storagesRes, usersRes, groupsRes] = await Promise.all([
      storageService.getStorages(),
      userService.getUsers(),
      groupService.getGroups()
    ])
    storages.value = storagesRes
    users.value = usersRes
    groups.value = groupsRes
  } catch (err: any) {
    error.value = err.message
  } finally {
    loading.value = false
  }
}

const getUserName = (id: number) => {
  const user = users.value.find(u => u.id === id)
  return user ? user.name : `ID: ${id}`
}

const getGroupName = (id: number) => {
  const group = groups.value.find(g => g.id === id)
  return group ? group.name : `ID: ${id}`
}

const openCreateModal = () => {
  modalMode.value = 'create'
  editingStorageId.value = null
  formData.value = {
    name: '',
    description: '',
    path: '',
    default_user: users.value[0]?.id || 1,
    default_group: groups.value[0]?.id || 1,
  }
  formErrors.value = {}
  showModal.value = true
}

const openEditModal = (storage: Storage) => {
  modalMode.value = 'edit'
  editingStorageId.value = storage.id
  formData.value = {
    name: storage.name,
    description: storage.description || '',
    path: storage.path,
    default_user: storage.default_user,
    default_group: storage.default_group,
  }
  formErrors.value = {}
  showModal.value = true
}

const closeModal = () => {
  showModal.value = false
  editingStorageId.value = null
  formErrors.value = {}
}

const validateForm = (): boolean => {
  formErrors.value = {}
  
  if (!formData.value.name.trim()) {
    formErrors.value.name = 'Name is required'
  }
  
  if (!formData.value.path.trim()) {
    formErrors.value.path = 'Path is required'
  }
  
  if (formData.value.default_user < 1) {
    formErrors.value.default_user = 'Default user is required'
  }
  
  if (formData.value.default_group < 1) {
    formErrors.value.default_group = 'Default group is required'
  }
  
  return Object.keys(formErrors.value).length === 0
}

const handleSubmit = async () => {
  if (!validateForm()) return
  
  try {
    submitting.value = true
    error.value = null
    
    if (modalMode.value === 'create') {
      const payload: CreateStorageRequest = {
        name: formData.value.name,
        path: formData.value.path,
        default_user: formData.value.default_user,
        default_group: formData.value.default_group,
      }
      if (formData.value.description) {
        payload.description = formData.value.description
      }
      await storageService.createStorage(payload)
    } else {
      const payload: UpdateStorageRequest = {
        name: formData.value.name,
        path: formData.value.path,
        default_user: formData.value.default_user,
        default_group: formData.value.default_group,
      }
      if (formData.value.description) {
        payload.description = formData.value.description
      }
      await storageService.updateStorage(editingStorageId.value!, payload)
    }
    
    await fetchData()
    closeModal()
  } catch (err: any) {
    error.value = err.message
  } finally {
    submitting.value = false
  }
}

const deleteStorage = async (storageId: number, name: string) => {
  if (!confirm(`Are you sure you want to delete storage "${name}"?`)) {
    return
  }
  
  try {
    error.value = null
    await storageService.deleteStorage(storageId)
    await fetchData()
  } catch (err: any) {
    error.value = err.message
  }
}

onMounted(() => {
  fetchData()
})
</script>

<template>
  <div class="storage-management">
    <div class="area-header">
      <div>
        <h2>Storage Management</h2>
        <p>Manage storage locations and configurations.</p>
      </div>
      <button class="btn-primary" @click="openCreateModal">
        <Plus :size="18" />
        <span>Create Storage</span>
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
      <p>Loading storages...</p>
    </div>
    
    <div v-else class="storages-table glass-panel">
      <table v-if="storages.length > 0">
        <thead>
          <tr>
            <th>Name</th>
            <th>Path</th>
            <th>Description</th>
            <th>Default User</th>
            <th>Default Group</th>
            <th>Actions</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="storage in storages" :key="storage.id">
            <td>
              <div class="storage-cell">
                <Database :size="16" />
                <span>{{ storage.name }}</span>
              </div>
            </td>
            <td>
              <span class="path">{{ storage.path }}</span>
            </td>
            <td>
              <span class="description">{{ storage.description || '-' }}</span>
            </td>
            <td>
              <span class="name-badge">{{ getUserName(storage.default_user) }}</span>
            </td>
            <td>
              <span class="name-badge">{{ getGroupName(storage.default_group) }}</span>
            </td>
            <td>
              <div class="actions">
                <button class="btn-icon" @click="openEditModal(storage)" title="Edit storage">
                  <Edit :size="16" />
                </button>
                <button class="btn-icon danger" @click="deleteStorage(storage.id, storage.name)" title="Delete storage">
                  <Trash2 :size="16" />
                </button>
              </div>
            </td>
          </tr>
        </tbody>
      </table>
      
      <div v-if="storages.length === 0" class="empty-state">
        <Database :size="48" style="opacity: 0.3" />
        <p>No storage locations found.</p>
      </div>
    </div>
    
    <!-- Modal -->
    <div v-if="showModal" class="modal-overlay" @click.self="closeModal">
      <div class="modal glass-panel">
        <div class="modal-header">
          <h3>{{ modalMode === 'create' ? 'Create Storage' : 'Edit Storage' }}</h3>
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
              placeholder="Storage name"
              :class="{ error: formErrors.name }"
            />
            <span v-if="formErrors.name" class="error-message">{{ formErrors.name }}</span>
          </div>
          
          <div class="form-group">
            <label for="path">Path *</label>
            <input
              id="path"
              v-model="formData.path"
              type="text"
              placeholder="/path/to/storage"
              :class="{ error: formErrors.path }"
            />
            <span v-if="formErrors.path" class="error-message">{{ formErrors.path }}</span>
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
            <div class="form-group">
              <label>Default User *</label>
              <UserSelect 
                v-model="formData.default_user" 
                :error="formErrors.default_user" 
              />
            </div>
            
            <div class="form-group">
              <label>Default Group *</label>
              <GroupSelect 
                v-model="formData.default_group" 
                :error="formErrors.default_group" 
              />
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
.storage-management {
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

.storages-table {
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

.storage-cell {
  display: flex;
  align-items: center;
  gap: 8px;
  font-weight: 500;
}

.path {
  font-family: monospace;
  color: var(--text-secondary);
  font-size: 0.875rem;
}

.description {
  color: var(--text-secondary);
  font-size: 0.875rem;
}

.name-badge {
  font-size: 0.75rem;
  padding: 4px 10px;
  border-radius: 20px;
  font-weight: 500;
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

.loading-state,
.empty-state {
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
  to {
    transform: rotate(360deg);
  }
}

/* Modal styles */
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
  max-width: 600px;
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
