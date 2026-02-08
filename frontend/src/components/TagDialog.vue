<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { X, Save, Tag as TagIcon } from 'lucide-vue-next'
import { tagService } from '../services/tag'
import { storageService } from '../services/storage'
import type { Tag, DirectoryEntry, Storage } from '../types'

const props = defineProps<{
  storage: Storage
  entry: DirectoryEntry
}>()

const emit = defineEmits(['close', 'updated'])

const allTags = ref<Tag[]>([])
const selectedEntryTags = ref<number[]>([])
const loading = ref(true)
const submitting = ref(false)

const fetchTags = async () => {
  try {
    loading.value = true
    allTags.value = await tagService.getTags()
    selectedEntryTags.value = [...(props.entry.tags || [])]
  } catch (err) {
    console.error('Failed to fetch tags:', err)
  } finally {
    loading.value = false
  }
}

const handleTagsSubmit = async () => {
  try {
    submitting.value = true
    await storageService.updateEntryTags(
      props.storage.id,
      props.entry.path,
      selectedEntryTags.value
    )
    emit('updated', [...selectedEntryTags.value])
    emit('close')
  } catch (err: any) {
    alert('Failed to update tags: ' + err.message)
  } finally {
    submitting.value = false
  }
}

const getBasename = (path: string) => {
  return path.split('/').filter(s => s).pop() || path
}

onMounted(fetchTags)
</script>

<template>
  <div class="modal-overlay" @click.self="emit('close')">
    <div class="modal glass-panel fade-in">
      <div class="modal-header">
        <div class="header-title">
          <TagIcon :size="20" class="header-icon" />
          <h3>Manage Tags</h3>
        </div>
        <button class="btn-icon" @click="emit('close')" :disabled="submitting">
          <X :size="20" />
        </button>
      </div>
      
      <div class="modal-body">
        <p class="modal-subtitle">Applying tags to: <strong>{{ getBasename(entry.path) }}</strong></p>
        
        <div v-if="loading" class="tags-loading">
          <div class="spinner-sm"></div>
          <span>Loading tags...</span>
        </div>
        
        <div v-else class="tags-selection-grid">
          <label 
            v-for="tag in allTags" 
            :key="tag.id" 
            class="tag-checkbox-item glass-panel" 
            :class="{ selected: selectedEntryTags.includes(tag.id) }"
          >
            <input 
              type="checkbox" 
              :value="tag.id" 
              v-model="selectedEntryTags"
              class="hidden-checkbox"
            />
            <TagIcon :size="16" />
            <span class="tag-name">{{ tag.name }}</span>
            <div class="check-indicator" v-if="selectedEntryTags.includes(tag.id)">
              <div class="dot"></div>
            </div>
          </label>
          
          <div v-if="allTags.length === 0" class="empty-tags-hint">
            <TagIcon :size="32" style="opacity: 0.1; margin-bottom: 10px;" />
            <p>No tags available.</p>
            <span class="hint">Create tags in the Tag Management section first.</span>
          </div>
        </div>
        
        <div class="modal-footer">
          <button type="button" class="btn-secondary" @click="emit('close')" :disabled="submitting">
            Cancel
          </button>
          <button type="button" class="btn-primary" @click="handleTagsSubmit" :disabled="submitting || loading">
            <Save :size="16" />
            <span>{{ submitting ? 'Saving...' : 'Save Tags' }}</span>
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.7);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 5000;
  padding: 20px;
  backdrop-filter: blur(8px);
}

.modal {
  width: 100%;
  max-width: 450px;
  box-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.3);
}

.modal-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 20px 24px;
  border-bottom: 1px solid var(--border-color);
}

.header-title {
  display: flex;
  align-items: center;
  gap: 12px;
}

.header-icon {
  color: var(--accent-color);
}

.modal-body {
  padding: 24px;
}

.modal-subtitle {
  font-size: 0.875rem;
  color: var(--text-secondary);
  margin-bottom: 20px;
  line-height: 1.5;
}

.tags-loading {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  padding: 40px;
  color: var(--text-secondary);
}

.tags-selection-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
  gap: 12px;
  max-height: 320px;
  overflow-y: auto;
  padding: 4px;
  margin-bottom: 24px;
  scrollbar-width: thin;
}

.tag-checkbox-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px;
  cursor: pointer;
  transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);
  border: 1px solid transparent;
  position: relative;
}

.tag-checkbox-item:hover {
  background: rgba(255, 255, 255, 0.08);
  transform: translateY(-2px);
}

.tag-checkbox-item.selected {
  background: rgba(59, 130, 246, 0.12);
  border-color: rgba(59, 130, 246, 0.4);
  color: var(--accent-color);
}

.tag-name {
  font-size: 0.875rem;
  font-weight: 500;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.check-indicator {
  position: absolute;
  top: 6px;
  right: 6px;
}

.dot {
  width: 6px;
  height: 6px;
  background: var(--accent-color);
  border-radius: 50%;
  box-shadow: 0 0 10px var(--accent-color);
}

.hidden-checkbox {
  display: none;
}

.empty-tags-hint {
  grid-column: 1 / -1;
  text-align: center;
  padding: 30px 20px;
  color: var(--text-secondary);
  background: rgba(255, 255, 255, 0.02);
  border-radius: 12px;
  border: 1px dashed var(--border-color);
}

.empty-tags-hint p {
  margin-bottom: 4px;
  color: var(--text-primary);
  font-weight: 500;
}

.empty-tags-hint .hint {
  font-size: 0.75rem;
  opacity: 0.7;
}

.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: 12px;
  margin-top: 8px;
}

.spinner-sm {
  width: 24px;
  height: 24px;
  border: 2px solid rgba(255, 255, 255, 0.1);
  border-top-color: var(--accent-color);
  border-radius: 50%;
  animation: spin 1s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}

.fade-in {
  animation: fadeIn 0.2s ease-out;
}

@keyframes fadeIn {
  from { opacity: 0; transform: scale(0.95); }
  to { opacity: 1; transform: scale(1); }
}
</style>
