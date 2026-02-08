<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import { 
  Folder, 
  File, 
  ChevronRight, 
  ArrowLeft, 
  Download, 
  Eye, 
  HardDrive,
  MoreVertical,
} from 'lucide-vue-next'
import { storageService } from '../services/storage'
import type { Storage, DirectoryEntry } from '../types'
import FileViewer from './FileViewer.vue'

const props = defineProps<{
  initialStorageId?: number
}>()

const storages = ref<Storage[]>([])
const selectedStorage = ref<Storage | null>(null)
const currentPath = ref<string>('')
const entries = ref<DirectoryEntry[]>([])
const loading = ref(false)
const error = ref<string | null>(null)
const selectedFile = ref<DirectoryEntry | null>(null)

const pathSegments = computed(() => {
  if (!currentPath.value) return []
  return currentPath.value.split('/').filter(s => s)
})

const fetchStorages = async () => {
  try {
    storages.value = await storageService.getStorages()
    if (props.initialStorageId) {
      const found = storages.value.find(s => s.id === props.initialStorageId)
      if (found) selectStorage(found)
    } else if (storages.value.length > 0) {
      selectStorage(storages.value[0])
    }
  } catch (err: any) {
    error.value = 'Failed to load storages: ' + err.message
  }
}

const selectStorage = (storage: Storage) => {
  selectedStorage.value = storage
  currentPath.value = ''
  fetchEntries()
}

const fetchEntries = async () => {
  if (!selectedStorage.value) return
  
  try {
    loading.value = true
    error.value = null
    const response = await storageService.listDirectory(selectedStorage.value.id, currentPath.value)
    entries.value = response.entries
  } catch (err: any) {
    error.value = 'Failed to load directory: ' + err.message
  } finally {
    loading.value = false
  }
}

const navigateTo = (path: string) => {
  currentPath.value = path
  fetchEntries()
}

const navigateUp = () => {
  const segments = pathSegments.value
  if (segments.length === 0) return
  const parentPath = segments.slice(0, -1).join('/')
  navigateTo(parentPath)
}

const formatSize = (bytes?: number) => {
  if (bytes === undefined) return '-'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let size = bytes
  let unitIdx = 0
  while (size >= 1024 && unitIdx < units.length - 1) {
    size /= 1024
    unitIdx++
  }
  return unitIdx === 0 ? `${bytes} B` : `${size.toFixed(1)} ${units[unitIdx]}`
}

const formatDate = (dateStr?: string) => {
  if (!dateStr) return '-'
  return new Date(dateStr).toLocaleString()
}

const getFileUrl = (entry: DirectoryEntry, mode: 'show' | 'download') => {
  if (!selectedStorage.value) return '#'
  return `/api/storage/${selectedStorage.value.id}/${mode}/${entry.path}`
}

onMounted(() => {
  fetchStorages()
})

const sortedEntries = computed(() => {
  return [...entries.value].sort((a, b) => {
    if (a.entry_type === 'Directory' && b.entry_type !== 'Directory') return -1
    if (a.entry_type !== 'Directory' && b.entry_type === 'Directory') return 1
    return a.path.localeCompare(b.path)
  })
})

const getBasename = (path: string) => {
  return path.split('/').filter(s => s).pop() || path
}

const isViewableAsText = (path: string) => {
  const ext = path.split('.').pop()?.toLowerCase()
  return [
    'txt', 'md', 'markdown', 'rs', 'js', 'mjs', 'ts', 'tsx', 'jsx', 'json', 
    'css', 'html', 'htm', 'toml', 'yaml', 'yml', 'xml', 'csv', 'ini',
    'py', 'java', 'c', 'cpp', 'cc', 'cxx', 'h', 'hpp', 'go', 'sh', 'rb', 
    'php', 'swift', 'kt', 'kts', 'vue', 'sql'
  ].includes(ext || '')
}

const handleEntryClick = (entry: DirectoryEntry) => {
  if (entry.entry_type === 'Directory') {
    navigateTo(entry.path)
  } else if (isViewableAsText(entry.path)) {
    selectedFile.value = entry
  }
}
</script>

<template>
  <div class="file-explorer">
    <div class="explorer-header">
      <div class="storage-selector glass-panel">
        <HardDrive :size="18" />
        <select v-model="selectedStorage" @change="selectStorage(selectedStorage!)" class="storage-select">
          <option v-for="s in storages" :key="s.id" :value="s">{{ s.name }}</option>
        </select>
      </div>

      <div class="breadcrumb-container glass-panel">
        <button class="btn-icon" @click="navigateTo('')" title="Root">
          <HardDrive :size="18" />
        </button>
        <ChevronRight :size="16" class="separator" />
        <div class="breadcrumbs">
          <template v-for="(segment, index) in pathSegments" :key="index">
            <span class="breadcrumb-item" @click="navigateTo(pathSegments.slice(0, index + 1).join('/'))">
              {{ segment }}
            </span>
            <ChevronRight v-if="index < pathSegments.length - 1" :size="16" class="separator" />
          </template>
        </div>
      </div>
    </div>

    <div v-if="error" class="error-state glass-panel fade-in">
      <p>{{ error }}</p>
      <button @click="fetchEntries" class="btn-primary">Retry</button>
    </div>

    <div v-else class="explorer-content glass-panel fade-in">
      <div class="table-header">
        <div class="col-name">Name</div>
        <div class="col-size">Size</div>
        <div class="col-date">Modified</div>
        <div class="col-actions"></div>
      </div>

      <div v-if="loading" class="loading-overlay">
        <div class="spinner"></div>
      </div>

      <div class="entries-list">
        <!-- Parent Directory -->
        <div v-if="currentPath" class="entry-row parent-dir" @click="navigateUp">
          <div class="col-name">
            <ArrowLeft :size="18" class="entry-icon" />
            <span>..</span>
          </div>
          <div class="col-size">-</div>
          <div class="col-date">-</div>
          <div class="col-actions"></div>
        </div>

        <!-- Entries -->
        <div v-for="entry in sortedEntries" :key="entry.path" class="entry-row" 
             @click="handleEntryClick(entry)">
          <div class="col-name">
            <Folder v-if="entry.entry_type === 'Directory'" :size="18" class="entry-icon folder" />
            <File v-else :size="18" class="entry-icon file" />
            <span class="entry-name">{{ getBasename(entry.path) }}</span>
          </div>
          <div class="col-size">{{ formatSize(entry.size) }}</div>
          <div class="col-date">{{ formatDate(entry.modified_at) }}</div>
          <div class="col-actions" @click.stop>
            <div class="action-buttons">
              <template v-if="entry.entry_type !== 'Directory'">
                <a :href="getFileUrl(entry, 'show')" target="_blank" class="btn-icon-sm" title="View">
                  <Eye :size="16" />
                </a>
                <a :href="getFileUrl(entry, 'download')" class="btn-icon-sm" title="Download">
                  <Download :size="16" />
                </a>
              </template>
              <button class="btn-icon-sm more">
                <MoreVertical :size="16" />
              </button>
            </div>
          </div>
        </div>

        <div v-if="entries.length === 0 && !loading" class="empty-directory">
          <Folder :size="48" style="opacity: 0.2" />
          <p>This directory is empty</p>
        </div>
      </div>
    </div>

    <!-- File Viewer Modal -->
    <FileViewer 
      v-if="selectedFile && selectedStorage" 
      :storage="selectedStorage"
      :entry="selectedFile"
      @close="selectedFile = null"
    />
  </div>
</template>

<style scoped>
.file-explorer {
  display: flex;
  flex-direction: column;
  gap: 20px;
  height: 100%;
}

.explorer-header {
  display: flex;
  gap: 16px;
  align-items: center;
}

.storage-selector {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 16px;
  min-width: 200px;
}

.storage-select {
  background: none;
  border: none;
  color: var(--text-primary);
  font-weight: 500;
  outline: none;
  width: 100%;
  cursor: pointer;
}

.storage-select option {
  background: #1a1a1a;
  color: white;
}

.breadcrumb-container {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  overflow: hidden;
}

.breadcrumbs {
  display: flex;
  align-items: center;
  gap: 8px;
  overflow-x: auto;
  scrollbar-width: none;
}

.breadcrumbs::-webkit-scrollbar {
  display: none;
}

.breadcrumb-item {
  cursor: pointer;
  color: var(--text-secondary);
  white-space: nowrap;
  transition: color 0.2s;
}

.breadcrumb-item:hover {
  color: var(--accent-color);
}

.breadcrumb-item:last-child {
  color: var(--text-primary);
  font-weight: 500;
  cursor: default;
}

.separator {
  color: var(--text-secondary);
  opacity: 0.5;
  flex-shrink: 0;
}

.explorer-content {
  flex: 1;
  display: flex;
  flex-direction: column;
  padding: 0;
  overflow: hidden;
  position: relative;
}

.table-header {
  display: grid;
  grid-template-columns: 1fr 120px 200px 100px;
  padding: 16px 24px;
  background: rgba(255, 255, 255, 0.02);
  border-bottom: 1px solid var(--border-color);
  font-weight: 600;
  font-size: 0.875rem;
  color: var(--text-secondary);
}

.entries-list {
  overflow-y: auto;
  flex: 1;
}

.entry-row {
  display: grid;
  grid-template-columns: 1fr 120px 200px 100px;
  padding: 12px 24px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.03);
  align-items: center;
  transition: background 0.2s;
  cursor: pointer;
}

.entry-row:hover {
  background: rgba(255, 255, 255, 0.03);
}

.col-name {
  display: flex;
  align-items: center;
  gap: 12px;
  overflow: hidden;
}

.entry-name {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  font-weight: 500;
}

.entry-icon.folder { color: #f59e0b; }
.entry-icon.file { color: #3b82f6; }

.col-size, .col-date {
  font-size: 0.875rem;
  color: var(--text-secondary);
}

.action-buttons {
  display: flex;
  gap: 4px;
  opacity: 0;
  transition: opacity 0.2s;
}

.entry-row:hover .action-buttons {
  opacity: 1;
}

.btn-icon-sm {
  padding: 6px;
  border-radius: 6px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  color: var(--text-secondary);
  cursor: pointer;
  transition: all 0.2s;
  display: inline-flex;
}

.btn-icon-sm:hover {
  background: rgba(59, 130, 246, 0.1);
  color: var(--accent-color);
  border-color: var(--accent-color);
}

.empty-directory {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 100px;
  gap: 16px;
  color: var(--text-secondary);
}

.loading-overlay {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.2);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 10;
  backdrop-filter: blur(2px);
}

.spinner {
  width: 32px;
  height: 32px;
  border: 3px solid rgba(255, 255, 255, 0.1);
  border-top-color: var(--accent-color);
  border-radius: 50%;
  animation: spin 1s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}

.fade-in {
  animation: fadeIn 0.3s ease-out;
}

@keyframes fadeIn {
  from { opacity: 0; transform: translateY(5px); }
  to { opacity: 1; transform: translateY(0); }
}
</style>
