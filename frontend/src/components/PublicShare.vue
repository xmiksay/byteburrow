<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import { useRoute } from 'vue-router'
import { 
  Cloud, 
  Folder, 
  FileText, 
  Image, 
  Music, 
  Video, 
  Download,
  Calendar,
  ChevronRight,
  Home,
  AlertCircle,
  Plus,
  Upload,
  Edit2,
  Trash2,
  Tag as TagIcon
} from 'lucide-vue-next'
import { storageService } from '../services/storage'
import { tagService } from '../services/tag'
import FileViewer from './FileViewer.vue'
import MediaViewer from './MediaViewer.vue'
import TagDialog from './TagDialog.vue'
import type { DirectoryEntry, Storage } from '../types'

const route = useRoute()
const token = route.params.token as string

const loading = ref(true)
const error = ref<string | null>(null)
const shareInfo = ref<any>(null)
const entries = ref<DirectoryEntry[]>([])
const currentPath = ref('')
const allTags = ref<any[]>([])
const entryForTags = ref<DirectoryEntry | null>(null)
const showTagsModal = ref(false)

// Viewers
const selectedFile = ref<DirectoryEntry | null>(null)
const selectedMediaFile = ref<DirectoryEntry | null>(null)

// Mock storage for viewers (needed for interface compatibility)
const mockStorage = computed<Storage>(() => ({
  id: 0,
  name: 'Shared Storage',
  path: '',
  description: '',
  default_user: 0,
  default_group: 0,
  ignore_patterns: ''
}))

const breadcrumbs = computed(() => {
  const parts = currentPath.value.split('/').filter(p => p)
  return [
    { name: 'Home', path: '' },
    ...parts.map((part, index) => ({
      name: part,
      path: parts.slice(0, index + 1).join('/')
    }))
  ]
})

const getBasename = (path: string) => {
  return path.split('/').filter(s => s).pop() || path
}

const getIcon = (entry: DirectoryEntry) => {
  if (entry.entry_type === 'Directory') return Folder
  
  const ext = entry.path.split('.').pop()?.toLowerCase() || ''
  if (['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg'].includes(ext)) return Image
  if (['mp3', 'wav', 'ogg', 'm4a'].includes(ext)) return Music
  if (['mp4', 'webm', 'ogv', 'mov', 'mkv'].includes(ext)) return Video
  return FileText
}

const formatDate = (date: string | undefined) => {
  if (!date) return '-'
  return new Date(date).toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric'
  })
}

const formatSize = (bytes: number | undefined) => {
  if (bytes === undefined || bytes === 0) return '0 B'
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i]
}

const fetchShareInfo = async () => {
  try {
    loading.value = true
    shareInfo.value = await storageService.getShareInfo(token)
    
    // Fetch tags and contents
    await Promise.all([
      fetchTags(),
      fetchContents('')
    ])
  } catch (err: any) {
    error.value = err.message || 'Failed to load shared content'
  } finally {
    loading.value = false
  }
}

const fetchTags = async () => {
  try {
    allTags.value = await tagService.getTags()
  } catch (err: any) {
    console.error('Failed to fetch tags:', err)
  }
}

const getTagName = (tagId: number) => {
  return allTags.value.find(t => t.id === tagId)?.name || `Tag ${tagId}`
}

const openTagsModal = (entry: DirectoryEntry) => {
  entryForTags.value = entry
  showTagsModal.value = true
}

const handleTagsUpdated = (newTags: number[]) => {
  if (entryForTags.value) {
    entryForTags.value.tags = newTags
  }
}

const fetchContents = async (path: string) => {
  try {
    loading.value = true
    error.value = null
    const response = await storageService.listShareDirectory(token, path)
    entries.value = response.entries
    currentPath.value = path
  } catch (err: any) {
    error.value = err.message || 'Failed to load directory contents'
    entries.value = []
  } finally {
    loading.value = false
  }
}

const handleNavigate = (path: string) => {
  if (path !== currentPath.value) {
    fetchContents(path)
  }
}

const handleEntryClick = (entry: DirectoryEntry) => {
  const name = getBasename(entry.path)
  
  if (entry.entry_type === 'Directory') {
    const newPath = currentPath.value ? `${currentPath.value}/${name}` : name
    handleNavigate(newPath)
  } else {
    // File
    const ext = entry.path.split('.').pop()?.toLowerCase() || ''
    if (['mp4', 'webm', 'ogv', 'mov', 'mkv', 'mp3', 'wav', 'ogg', 'm4a', 'flac'].includes(ext)) {
      selectedMediaFile.value = entry
    } else {
      selectedFile.value = entry
    }
  }
}

const handleDownload = (entry: DirectoryEntry) => {
  const url = `/api/storage/share/${token}/show/${entry.path}` 
  window.open(url, '_blank')
}

// Write operations
const isWriting = ref(false)
const fileInput = ref<HTMLInputElement | null>(null)

const canWrite = computed(() => shareInfo.value?.can_write)

// Reflect tag changes made inside FileViewer back into the entries list and
// the currently-selected file ref (which FileViewer renders).
const handleViewerTagsUpdated = (newTags: number[]) => {
  if (!selectedFile.value) return
  const path = selectedFile.value.path
  selectedFile.value.tags = newTags
  const entry = entries.value.find(e => e.path === path)
  if (entry) {
    entry.tags = newTags
  }
}

const handleCreateFolder = async () => {
  const name = prompt('Folder name:')
  if (!name) return

  try {
    isWriting.value = true
    const parent = currentPath.value
    const path = parent ? `${parent}/${name}` : name
    await storageService.createShareEntry(token, path, 'Directory')
    fetchContents(currentPath.value)
  } catch (err: any) {
    alert(err.message)
  } finally {
    isWriting.value = false
  }
}

const triggerUpload = () => {
  fileInput.value?.click()
}

const handleUpload = async (event: Event) => {
  const input = event.target as HTMLInputElement
  if (!input.files?.length) return

  const file = input.files[0]
  const parent = currentPath.value
  const path = parent ? `${parent}/${file.name}` : file.name

  try {
    isWriting.value = true
    // Create entry first
    await storageService.createShareEntry(token, path, 'File')
    // Then upload content
    await storageService.updateShareFileContent(token, path, file)
    fetchContents(currentPath.value)
  } catch (err: any) {
    alert('Upload failed: ' + err.message)
  } finally {
    isWriting.value = false
    input.value = ''
  }
}

const handleRename = async (entry: DirectoryEntry) => {
  const oldName = getBasename(entry.path)
  const newName = prompt('New name:', oldName)
  if (!newName || newName === oldName) return

  const parent = entry.path.substring(0, entry.path.lastIndexOf('/'))
  const newPath = parent ? `${parent}/${newName}` : newName

  try {
    isWriting.value = true
    await storageService.renameShareEntry(token, entry.path, newPath)
    fetchContents(currentPath.value)
  } catch (err: any) {
    alert('Rename failed: ' + err.message)
  } finally {
    isWriting.value = false
  }
}

const handleDelete = async (entry: DirectoryEntry) => {
  if (!confirm(`Delete ${getBasename(entry.path)}?`)) return

  try {
    isWriting.value = true
    await storageService.removeShareEntry(token, entry.path)
    fetchContents(currentPath.value)
  } catch (err: any) {
    alert('Delete failed: ' + err.message)
  } finally {
    isWriting.value = false
  }
}

onMounted(() => {
  fetchShareInfo()
})
</script>

<template>
  <div class="public-share">
    <header class="share-header glass-panel">
      <div class="header-content">
        <div class="logo">
          <Cloud :size="24" class="text-accent" />
          <h1>Cloud Share</h1>
        </div>
        <div class="share-meta" v-if="shareInfo">
          <div class="meta-item">
            <span class="label">Shared Item:</span>
            <span class="value">{{ shareInfo.name || 'Folder' }}</span>
          </div>
          <div class="meta-item" v-if="shareInfo.expires_at">
            <Calendar :size="14" />
            <span class="value">Expires: {{ formatDate(shareInfo.expires_at) }}</span>
          </div>
        </div>
      </div>
    </header>

    <div class="main-content">
      <div v-if="error" class="error-banner glass-panel">
        <AlertCircle :size="20" />
        <p>{{ error }}</p>
      </div>

      <div v-else class="browser-panel glass-panel">
        <!-- Breadcrumbs -->
        <div class="breadcrumbs">
          <button 
            v-for="(crumb, index) in breadcrumbs" 
            :key="index"
            class="crumb-btn"
            :class="{ active: index === breadcrumbs.length - 1 }"
            @click="handleNavigate(crumb.path)"
          >
            <Home v-if="index === 0" :size="16" />
            <span v-else>{{ crumb.name }}</span>
            <ChevronRight v-if="index < breadcrumbs.length - 1" :size="14" class="separator" />
          </button>

          <div style="flex: 1"></div>

          <div v-if="canWrite" class="toolbar-actions">
            <input type="file" ref="fileInput" class="hidden" @change="handleUpload" />
            <button class="btn-icon" @click="handleCreateFolder" title="New Folder" :disabled="isWriting">
               <Plus :size="18" />
            </button>
            <button class="btn-icon" @click="triggerUpload" title="Upload File" :disabled="isWriting">
               <Upload :size="18" />
            </button>
          </div>
        </div>

        <!-- File List -->
        <div class="file-list">
          <div v-if="loading && entries.length === 0" class="loading-state">
            <div class="spinner"></div>
            <p>Loading contents...</p>
          </div>
          
          <div v-else-if="entries.length === 0" class="empty-state">
            <Folder :size="48" style="opacity: 0.2" />
            <p>This folder is empty</p>
          </div>

          <table v-else>
            <thead>
              <tr>
                <th>Name</th>
                <th>Size</th>
                <th>Modified</th>
                <th class="actions-col"></th>
              </tr>
            </thead>
            <tbody>
              <tr 
                v-for="entry in entries" 
                :key="entry.path"
                @click="handleEntryClick(entry)"
                class="clickable-row"
              >
                <td class="name-cell">
                  <component :is="getIcon(entry)" :size="20" class="file-icon" />
                  <span class="entry-name">{{ getBasename(entry.path) }}</span>
                  <div class="entry-tags" v-if="entry.tags?.length">
                    <span v-for="tagId in entry.tags" :key="tagId" class="tag-pill">
                      {{ getTagName(tagId) }}
                    </span>
                  </div>
                </td>
                <td class="size-cell">{{ entry.entry_type === 'File' ? formatSize(entry.size) : '-' }}</td>
                <td class="date-cell">{{ formatDate(entry.modified_at || entry.created_at) }}</td>
                <td class="actions-col" @click.stop>
                  <button 
                    v-if="entry.entry_type === 'File'"
                    class="btn-icon" 
                    @click="handleDownload(entry)"
                    title="Download"
                  >
                    <Download :size="16" />
                  </button>
                  <template v-if="canWrite">
                    <button class="btn-icon" @click.stop="handleRename(entry)" title="Rename" :disabled="isWriting">
                      <Edit2 :size="16" />
                    </button>
                    <button class="btn-icon" @click.stop="openTagsModal(entry)" title="Manage Tags" :disabled="isWriting">
                      <TagIcon :size="16" />
                    </button>
                    <button class="btn-icon danger" @click.stop="handleDelete(entry)" title="Delete" :disabled="isWriting">
                      <Trash2 :size="16" />
                    </button>
                  </template>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </div>

    <!-- Viewers -->
    <!-- Note: passing token as share-id works because logic handles string token now -->
    <FileViewer
      v-if="selectedFile"
      :storage="mockStorage"
      :entry="selectedFile"
      :share-id="token" 
      :read-only="!canWrite"
      @close="selectedFile = null"
      @tags-updated="handleViewerTagsUpdated"
    />

    <MediaViewer
      v-if="selectedMediaFile"
      :storage="mockStorage"
      :entry="selectedMediaFile"
      :share-id="token"
      @close="selectedMediaFile = null"
    />

    <!-- Tags Dialog -->
    <TagDialog
      v-if="showTagsModal && entryForTags"
      :storage="mockStorage"
      :entry="entryForTags"
      :share-id="token"
      @close="showTagsModal = false"
      @updated="handleTagsUpdated"
    />
  </div>
</template>

<style scoped>
.public-share {
  min-height: 100vh;
  background: var(--bg-color);
  display: flex;
  flex-direction: column;
}

.share-header {
  padding: 16px 24px;
  border-bottom: 1px solid var(--border-color);
  background: rgba(30, 30, 30, 0.6);
  backdrop-filter: blur(12px);
  position: sticky;
  top: 0;
  z-index: 10;
}

.header-content {
  max-width: 1200px;
  margin: 0 auto;
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.logo {
  display: flex;
  align-items: center;
  gap: 12px;
}

.logo h1 {
  font-size: 1.25rem;
  font-weight: 600;
  color: var(--text-primary);
  margin: 0;
}

.text-accent {
  color: var(--accent-color);
}

.share-meta {
  display: flex;
  gap: 24px;
  align-items: center;
}

.meta-item {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 0.9rem;
  color: var(--text-secondary);
}

.meta-item .value {
  color: var(--text-primary);
  font-weight: 500;
}

.main-content {
  flex: 1;
  padding: 24px;
  max-width: 1200px;
  margin: 0 auto;
  width: 100%;
}

.browser-panel {
  display: flex;
  flex-direction: column;
  height: calc(100vh - 120px);
  overflow: hidden;
}

.breadcrumbs {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 16px;
  border-bottom: 1px solid var(--border-color);
  background: rgba(255, 255, 255, 0.02);
  overflow-x: auto;
}

.crumb-btn {
  display: flex;
  align-items: center;
  gap: 4px;
  background: none;
  border: none;
  color: var(--text-secondary);
  cursor: pointer;
  padding: 4px 8px;
  border-radius: 4px;
  font-size: 0.9rem;
  transition: all 0.2s;
}

.crumb-btn:hover {
  background: rgba(255, 255, 255, 0.05);
  color: var(--text-primary);
}

.crumb-btn.active {
  color: var(--accent-color);
  font-weight: 500;
}

.separator {
  color: var(--text-secondary);
  opacity: 0.5;
}

.file-list {
  flex: 1;
  overflow-y: auto;
}

table {
  width: 100%;
  border-collapse: collapse;
}

th {
  text-align: left;
  padding: 12px 16px;
  font-weight: 500;
  color: var(--text-secondary);
  font-size: 0.85rem;
  border-bottom: 1px solid var(--border-color);
  position: sticky;
  top: 0;
  background: #1e1e1e;
  z-index: 1;
}

td {
  padding: 12px 16px;
  border-bottom: 1px solid var(--border-color);
  color: var(--text-primary);
}

.clickable-row:hover {
  background: rgba(255, 255, 255, 0.03);
  cursor: pointer;
}

.name-cell {
  display: flex;
  align-items: center;
  gap: 12px;
}

.file-icon {
  color: var(--text-secondary);
  flex-shrink: 0;
}

.entry-name {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 300px;
}

.entry-tags {
  display: flex;
  gap: 4px;
  overflow: hidden;
  mask-image: linear-gradient(to right, black 85%, transparent 100%);
  margin-left: 8px;
}

.tag-pill {
  font-size: 0.65rem;
  padding: 2px 6px;
  background: rgba(59, 130, 246, 0.15);
  color: var(--accent-color);
  border-radius: 4px;
  white-space: nowrap;
  border: 1px solid rgba(59, 130, 246, 0.2);
}

.size-cell, .date-cell {
  color: var(--text-secondary);
  font-size: 0.9rem;
  width: 150px;
}

.actions-col {
  width: 120px;
  text-align: right;
}

.btn-icon {
  background: none;
  border: none;
  color: var(--text-secondary);
  cursor: pointer;
  padding: 6px;
  border-radius: 4px;
  transition: all 0.2s;
  display: inline-flex;
}

.btn-icon:hover {
  background: rgba(255, 255, 255, 0.1);
  color: var(--text-primary);
}

.loading-state, .empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 60px;
  gap: 16px;
  color: var(--text-secondary);
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

.error-banner {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 16px;
  background: rgba(239, 68, 68, 0.1);
  border: 1px solid rgba(239, 68, 68, 0.3);
  border-radius: 8px;
  color: #ef4444;
}

.toolbar-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.hidden {
  display: none;
}

.btn-icon.danger:hover {
  color: #ef4444;
  background: rgba(239, 68, 68, 0.1);
}
</style>
