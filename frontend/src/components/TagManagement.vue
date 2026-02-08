<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { Tag as TagIcon, Plus, Trash2, Edit2, X, Save } from 'lucide-vue-next'
import { tagService } from '../services/tag'
import { useAuth } from '../composables/useAuth'
import type { Tag, CreateTagRequest, UpdateTagRequest } from '../types'

const { user } = useAuth()

const tags = ref<Tag[]>([])
const loading = ref(false)
const error = ref<string | null>(null)

// Modal state
const showModal = ref(false)
const modalMode = ref<'create' | 'edit'>('create')
const formData = ref<CreateTagRequest>({ name: '', description: '' })
const currentTagId = ref<number | null>(null)
const submitting = ref(false)

const fetchTags = async () => {
  try {
    loading.value = true
    error.value = null
    tags.value = await tagService.getTags()
  } catch (err: any) {
    error.value = 'Failed to load tags: ' + err.message
  } finally {
    loading.value = false
  }
}

const openCreateModal = () => {
  modalMode.value = 'create'
  formData.value = { name: '', description: '' }
  showModal.value = true
}

const openEditModal = (tag: Tag) => {
  modalMode.value = 'edit'
  currentTagId.value = tag.id
  formData.value = { name: tag.name, description: tag.description || '' }
  showModal.value = true
}

const handleSubmit = async () => {
  if (!formData.value.name.trim()) return
  
  try {
    submitting.value = true
    if (modalMode.value === 'create') {
      await tagService.createTag(formData.value)
    } else if (currentTagId.value !== null) {
      await tagService.updateTag(currentTagId.value, formData.value as UpdateTagRequest)
    }
    
    await fetchTags()
    showModal.value = false
  } catch (err: any) {
    alert('Operation failed: ' + err.message)
  } finally {
    submitting.value = false
  }
}

const deleteTag = async (tag: Tag) => {
  if (!confirm(`Are you sure you want to delete tag "${tag.name}"?`)) return
  
  try {
    await tagService.deleteTag(tag.id)
    await fetchTags()
  } catch (err: any) {
    alert('Failed to delete: ' + err.message)
  }
}

onMounted(fetchTags)
</script>

<template>
  <div class="management-container fade-in">
    <div class="area-header">
      <div class="header-content">
        <h2>Tag Management</h2>
        <p>Organize tags to label your files.</p>
      </div>
      <button v-if="user?.admin" class="btn-primary" @click="openCreateModal">
        <Plus :size="18" />
        <span>Create Tag</span>
      </button>
    </div>

    <div v-if="loading && tags.length === 0" class="loading-state">
      <div class="spinner"></div>
      <p>Loading tags...</p>
    </div>

    <div v-else-if="error" class="error-state glass-panel">
      <p>{{ error }}</p>
      <button @click="fetchTags" class="btn-primary">Retry</button>
    </div>

    <div v-else class="grid-container">
      <div v-for="tag in tags" :key="tag.id" class="data-card glass-panel fade-in">
        <div class="card-header">
          <div class="tag-badge">
            <TagIcon :size="16" />
            <h3>{{ tag.name }}</h3>
          </div>
          <span class="id-tag">#{{ tag.id }}</span>
        </div>
        
        <p class="description">{{ tag.description || 'No description' }}</p>
        
        <div v-if="user?.admin" class="card-footer">
          <div class="card-actions">
            <button class="btn-icon-sm" @click="openEditModal(tag)" title="Edit Tag">
              <Edit2 :size="16" />
            </button>
            <button class="btn-icon-sm danger" @click="deleteTag(tag)" title="Delete Tag">
              <Trash2 :size="16" />
            </button>
          </div>
        </div>
      </div>

      <div v-if="tags.length === 0" class="empty-state glass-panel fade-in">
        <TagIcon :size="48" style="opacity: 0.2" />
        <p>{{ user?.admin ? "You haven't created any tags yet." : "No tags exist yet." }}</p>
        <button v-if="user?.admin" class="btn-primary" @click="openCreateModal">Create your first tag</button>
      </div>
    </div>

    <!-- Create/Edit Modal -->
    <div v-if="showModal" class="modal-overlay" @click.self="showModal = false">
      <div class="modal glass-panel">
        <div class="modal-header">
          <h3>{{ modalMode === 'create' ? 'Create New Tag' : 'Edit Tag' }}</h3>
          <button class="btn-icon" @click="showModal = false">
            <X :size="20" />
          </button>
        </div>
        
        <form @submit.prevent="handleSubmit" class="modal-body">
          <div class="form-group">
            <label for="tagName">Tag Name</label>
            <input
              id="tagName"
              v-model="formData.name"
              type="text"
              placeholder="e.g. Work, Personal, Important"
              required
              autofocus
            />
          </div>
          
          <div class="form-group">
            <label for="tagDesc">Description (Optional)</label>
            <textarea
              id="tagDesc"
              v-model="formData.description"
              placeholder="What is this tag for?"
              rows="3"
            ></textarea>
          </div>
          
          <div class="modal-footer">
            <button type="button" class="btn-secondary" @click="showModal = false" :disabled="submitting">
              Cancel
            </button>
            <button type="submit" class="btn-primary" :disabled="submitting || !formData.name.trim()">
              <Save :size="16" />
              <span>{{ submitting ? 'Saving...' : 'Save Tag' }}</span>
            </button>
          </div>
        </form>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* Inherit styles from common management components or define locally */
.management-container {
  display: flex;
  flex-direction: column;
  gap: 30px;
}

.area-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.tag-badge {
  display: flex;
  align-items: center;
  gap: 10px;
  color: var(--accent-color);
}

.tag-badge h3 {
  color: var(--text-primary);
  margin: 0;
}

.grid-container {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
  gap: 20px;
}

.data-card {
  padding: 24px;
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.card-header {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
}

.description {
  font-size: 0.875rem;
  color: var(--text-secondary);
  line-height: 1.5;
}

.card-actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
}

.loading-state, .empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 100px;
  gap: 20px;
  text-align: center;
}

/* Modal styles match the rest of the app */
.modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.7);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 3000;
  padding: 20px;
  backdrop-filter: blur(4px);
}

.modal {
  width: 100%;
  max-width: 450px;
}

.modal-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 20px 24px;
  border-bottom: 1px solid var(--border-color);
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
  color: var(--text-secondary);
}

.form-group input, .form-group textarea {
  padding: 10px 12px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  color: var(--text-primary);
  outline: none;
}

.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: 12px;
}
</style>
