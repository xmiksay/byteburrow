<script setup lang="ts">
import { ref, onMounted, computed, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { 
  Folder,
  File, 
  ChevronRight, 
  ArrowLeft, 
  Download, 
  Eye, 
  HardDrive,
  FilePlus,
  FolderPlus,
  Trash2,
  Edit2,
  X,
  Save,
  Link,
  Tag as TagIcon,
  Share2,
  Hash,
  Search
} from 'lucide-vue-next'
import { storageService } from '../services/storage'
import { tagService } from '../services/tag'
import { useAuth } from '../composables/useAuth'
import type { Storage, DirectoryEntry, Tag } from '../types'
import { formatSize, formatDate, getBasename, isViewableAsText, isImage, isMediaFile, getThumbnailUrl, sortEntries } from '../utils/file'
import FileViewer from './FileViewer.vue'
import MediaViewer from './MediaViewer.vue'
import ShareDialog from './ShareDialog.vue'

const { user } = useAuth()
const route = useRoute()
const router = useRouter()

const storages = ref<Storage[]>([])
const selectedStorage = ref<Storage | null>(null)
const currentPath = ref<string>('')
const entries = ref<DirectoryEntry[]>([])
const loading = ref(false)
const error = ref<string | null>(null)
const selectedFile = ref<DirectoryEntry | null>(null)
const selectedMediaFile = ref<DirectoryEntry | null>(null)

// Modal state
const showModal = ref(false)
const modalMode = ref<'create-file' | 'create-folder' | 'rename'>('create-file')
const itemName = ref('')
const itemToRename = ref<DirectoryEntry | null>(null)
const submitting = ref(false)

// Tags state
const allTags = ref<Tag[]>([])
const showTagsModal = ref(false)
const entryToTag = ref<DirectoryEntry | null>(null)
const selectedEntryTags = ref<number[]>([])

// Share state
const showShareModal = ref(false)
const entryToShare = ref<DirectoryEntry | null>(null)

// Filter state
const filterName = ref('')
const filterTags = ref<number[]>([])

const toggleFilterTag = (tagId: number) => {
  const idx = filterTags.value.indexOf(tagId)
  if (idx === -1) {
    filterTags.value.push(tagId)
  } else {
    filterTags.value.splice(idx, 1)
  }
}

const pathSegments = computed(() => {
  if (!currentPath.value) return []
  return currentPath.value.split('/').filter(s => s)
})

const fetchStorages = async () => {
  try {
    storages.value = await storageService.getStorages()

    const routeStorageId = route.params.storageId ? Number(route.params.storageId) : undefined
    const routePath = (route.params.path as string) || ''

    if (routeStorageId) {
      const found = storages.value.find(s => s.id === routeStorageId)
      if (found) {
        selectedStorage.value = found
        currentPath.value = routePath
        fetchEntries()
        return
      }
    }

    if (storages.value.length > 0) {
      // Redirect to first storage URL
      router.replace({ name: 'files', params: { storageId: storages.value[0].id, path: '' } })
    }
  } catch (err: any) {
    error.value = 'Failed to load storages: ' + err.message
  }
}

const selectStorage = (storage: Storage) => {
  router.push({ name: 'files', params: { storageId: storage.id, path: '' } })
}

const onStorageSelect = (event: Event) => {
  const id = Number((event.target as HTMLSelectElement).value)
  const storage = storages.value.find(s => s.id === id)
  if (storage) selectStorage(storage)
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
  if (!selectedStorage.value) return
  router.push({ name: 'files', params: { storageId: selectedStorage.value.id, path } })
}

const navigateUp = () => {
  const segments = pathSegments.value
  if (segments.length === 0) return
  const parentPath = segments.slice(0, -1).join('/')
  navigateTo(parentPath)
}

// React to route changes (back/forward navigation, programmatic pushes)
watch(
  () => [route.params.storageId, route.params.path],
  ([newStorageId, newPath]) => {
    if (route.name !== 'files') return
    const id = newStorageId ? Number(newStorageId) : undefined
    const path = (newPath as string) || ''

    if (id && id !== selectedStorage.value?.id) {
      const found = storages.value.find(s => s.id === id)
      if (found) {
        selectedStorage.value = found
        currentPath.value = path
        fetchEntries()
      }
    } else if (path !== currentPath.value) {
      currentPath.value = path
      fetchEntries()
    }
  }
)

const getFileUrl = (entry: DirectoryEntry, mode: 'show' | 'download') => {
  if (!selectedStorage.value) return '#'
  return `/api/storage/${selectedStorage.value.id}/${mode}/${entry.path}`
}

const fetchTags = async () => {
  try {
    allTags.value = await tagService.getTags()
  } catch (err: any) {
    console.error('Failed to fetch tags:', err)
  }
}

const openTagsModal = (entry: DirectoryEntry) => {
  entryToTag.value = entry
  selectedEntryTags.value = [...(entry.tags || [])]
  showTagsModal.value = true
}

const openShareModal = (entry: DirectoryEntry) => {
  entryToShare.value = entry
  showShareModal.value = true
}

const handleTagsSubmit = async () => {
  if (!entryToTag.value || !selectedStorage.value) return
  
  try {
    submitting.value = true
    await storageService.updateEntryTags(
      selectedStorage.value.id, 
      entryToTag.value.path, 
      selectedEntryTags.value
    )
    await fetchEntries()
    showTagsModal.value = false
  } catch (err: any) {
    alert('Failed to update tags: ' + err.message)
  } finally {
    submitting.value = false
  }
}

const getTagName = (tagId: number) => {
  return allTags.value.find(t => t.id === tagId)?.name || `Tag ${tagId}`
}

onMounted(() => {
  fetchStorages()
  fetchTags()
})

const sortedEntries = computed(() => {
  let result = entries.value
  const nameQuery = filterName.value.trim().toLowerCase()
  if (nameQuery) {
    result = result.filter(e => getBasename(e.path).toLowerCase().includes(nameQuery))
  }
  if (filterTags.value.length > 0) {
    result = result.filter(e => filterTags.value.every(t => e.tags.includes(t)))
  }
  return sortEntries(result)
})

const handleEntryClick = (entry: DirectoryEntry) => {
  if (entry.entry_type === 'Directory' || (entry.entry_type === 'Symlink' && !isMediaFile(entry.path) && !isViewableAsText(entry.path))) {
    navigateTo(entry.path)
  } else if (isViewableAsText(entry.path)) {
    selectedFile.value = entry
  } else if (isMediaFile(entry.path)) {
    selectedMediaFile.value = entry
  }
}

const openCreateModal = (mode: 'create-file' | 'create-folder') => {
  modalMode.value = mode
  itemName.value = ''
  showModal.value = true
}

const openRenameModal = (entry: DirectoryEntry) => {
  modalMode.value = 'rename'
  itemToRename.value = entry
  itemName.value = getBasename(entry.path)
  showModal.value = true
}

const handleModalSubmit = async () => {
  if (!itemName.value.trim() || !selectedStorage.value) return
  
  try {
    submitting.value = true
    if (modalMode.value === 'create-file') {
      const path = currentPath.value ? `${currentPath.value}/${itemName.value}` : itemName.value
      await storageService.createEntry(selectedStorage.value.id, path, 'File')
    } else if (modalMode.value === 'create-folder') {
      const path = currentPath.value ? `${currentPath.value}/${itemName.value}` : itemName.value
      await storageService.createEntry(selectedStorage.value.id, path, 'Directory')
    } else if (modalMode.value === 'rename' && itemToRename.value) {
      const entry = itemToRename.value
      const basename = getBasename(entry.path)
      const parent = entry.path.substring(0, entry.path.length - basename.length)
      const newPath = `${parent}${itemName.value}`
      await storageService.renameEntry(selectedStorage.value.id, entry.path, newPath)
    }
    
    await fetchEntries()
    showModal.value = false
  } catch (err: any) {
    alert('Operation failed: ' + err.message)
  } finally {
    submitting.value = false
  }
}

const triggerHash = async (entry: DirectoryEntry) => {
  if (!selectedStorage.value) return
  try {
    await storageService.triggerHash(selectedStorage.value.id, entry.path)
  } catch (err: any) {
    alert('Failed to trigger hash: ' + err.message)
  }
}

const deleteEntry = async (entry: DirectoryEntry) => {
  if (!confirm(`Are you sure you want to delete ${getBasename(entry.path)}?`)) return
  if (!selectedStorage.value) return
  
  try {
    await storageService.removeEntry(selectedStorage.value.id, entry.path)
    await fetchEntries()
  } catch (err: any) {
    alert('Failed to delete: ' + err.message)
  }
}
</script>

<template>
  <div class="file-explorer">
    <div class="explorer-header">
      <div class="storage-selector glass-panel">
        <HardDrive :size="18" />
        <select :value="selectedStorage?.id" @change="onStorageSelect($event)" class="storage-select">
          <option v-for="s in storages" :key="s.id" :value="s.id">{{ s.name }}</option>
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

      <div class="header-actions">
        <button class="btn-secondary" @click="openCreateModal('create-file')" title="New File">
          <FilePlus :size="18" />
          <span>New File</span>
        </button>
        <button class="btn-secondary" @click="openCreateModal('create-folder')" title="New Folder">
          <FolderPlus :size="18" />
          <span>New Folder</span>
        </button>
      </div>
    </div>

    <div v-if="entries.length > 0 || allTags.length > 0" class="filter-bar glass-panel">
      <div class="filter-input-wrapper">
        <Search :size="16" class="filter-input-icon" />
        <input
          v-model="filterName"
          type="text"
          class="filter-input"
          placeholder="Filter by name..."
        />
      </div>
      <div v-if="allTags.length > 0" class="filter-tags">
        <span
          v-for="tag in allTags"
          :key="tag.id"
          class="filter-tag"
          :class="{ active: filterTags.includes(tag.id) }"
          @click="toggleFilterTag(tag.id)"
        >
          {{ tag.name }}
        </span>
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
            <Link v-else-if="entry.entry_type === 'Symlink'" :size="18" class="entry-icon symlink" />
            <div v-else-if="isImage(entry.path) && entry.hash" class="entry-thumbnail-container">
              <img :src="getThumbnailUrl(entry, 'mini')!" :alt="getBasename(entry.path)" class="entry-thumbnail" @error="(e) => (e.target as HTMLImageElement).style.display = 'none'" />
              <File :size="18" class="entry-icon file fallback-icon" />
            </div>
            <File v-else :size="18" class="entry-icon file" />
            <span class="entry-name">{{ getBasename(entry.path) }}</span>
            <div class="entry-tags" v-if="entry.tags?.length > 0">
              <span v-for="tagId in entry.tags" :key="tagId" class="tag-pill">
                {{ getTagName(tagId) }}
              </span>
            </div>
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
              <button class="btn-icon-sm" @click="openRenameModal(entry)" title="Rename">
                <Edit2 :size="16" />
              </button>
              <button class="btn-icon-sm danger" @click="deleteEntry(entry)" title="Delete">
                <Trash2 :size="16" />
              </button>
              <button v-if="user?.admin" class="btn-icon-sm" @click="openTagsModal(entry)" title="Manage Tags">
                <TagIcon :size="16" />
              </button>
              <button v-if="entry.entry_type !== 'Directory'" class="btn-icon-sm" @click="triggerHash(entry)" title="Calculate Hash">
                <Hash :size="16" />
              </button>
              <button class="btn-icon-sm share" @click="openShareModal(entry)" title="Share">
                <Share2 :size="16" />
              </button>
            </div>
          </div>
        </div>

        <div v-if="sortedEntries.length === 0 && !loading" class="empty-directory">
          <template v-if="entries.length === 0">
            <Folder :size="48" style="opacity: 0.2" />
            <p>This directory is empty</p>
          </template>
          <template v-else>
            <Search :size="48" style="opacity: 0.2" />
            <p>No matching files</p>
          </template>
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

    <!-- Media Viewer Modal -->
    <MediaViewer
      v-if="selectedMediaFile && selectedStorage"
      :storage="selectedStorage"
      :entry="selectedMediaFile"
      @close="selectedMediaFile = null"
    />

    <!-- Creation/Rename Modal -->
    <div v-if="showModal" class="modal-overlay" @click.self="showModal = false">
      <div class="modal glass-panel">
        <div class="modal-header">
          <h3>
            <template v-if="modalMode === 'create-file'">Create New File</template>
            <template v-else-if="modalMode === 'create-folder'">Create New Folder</template>
            <template v-else>Rename Entry</template>
          </h3>
          <button class="btn-icon" @click="showModal = false">
            <X :size="20" />
          </button>
        </div>
        
        <form @submit.prevent="handleModalSubmit" class="modal-body">
          <div class="form-group">
            <label for="itemName">Name</label>
            <input
              id="itemName"
              v-model="itemName"
              type="text"
              :placeholder="modalMode === 'create-folder' ? 'Folder name' : 'File name'"
              autofocus
            />
          </div>
          
          <div class="modal-footer">
            <button type="button" class="btn-secondary" @click="showModal = false" :disabled="submitting">
              Cancel
            </button>
            <button type="submit" class="btn-primary" :disabled="submitting || !itemName.trim()">
              <Save :size="16" />
              <span>{{ submitting ? 'Processing...' : 'Confirm' }}</span>
            </button>
          </div>
        </form>
      </div>
    </div>

    <!-- Tags Modal -->
    <div v-if="showTagsModal && entryToTag" class="modal-overlay" @click.self="showTagsModal = false">
      <div class="modal glass-panel fade-in">
        <div class="modal-header">
          <div class="header-title">
            <TagIcon :size="20" class="header-icon" />
            <h3>Manage Tags</h3>
          </div>
          <button class="btn-icon" @click="showTagsModal = false">
            <X :size="20" />
          </button>
        </div>
        
        <div class="modal-body">
          <p class="modal-subtitle">Applying tags to: <strong>{{ getBasename(entryToTag.path) }}</strong></p>
          
          <div class="tags-selection-grid">
            <label v-for="tag in allTags" :key="tag.id" class="tag-checkbox-item glass-panel" :class="{ selected: selectedEntryTags.includes(tag.id) }">
              <input 
                type="checkbox" 
                :value="tag.id" 
                v-model="selectedEntryTags"
                class="hidden-checkbox"
              />
              <TagIcon :size="16" />
              <span>{{ tag.name }}</span>
            </label>
            
            <div v-if="allTags.length === 0" class="empty-tags-hint">
              <TagIcon :size="32" style="opacity: 0.1; margin-bottom: 10px;" />
              <p>No tags available.</p>
              <span class="hint">Create tags in the Tag Management section first.</span>
            </div>
          </div>
          
          <div class="modal-footer">
            <button type="button" class="btn-secondary" @click="showTagsModal = false" :disabled="submitting">
              Cancel
            </button>
            <button type="button" class="btn-primary" @click="handleTagsSubmit" :disabled="submitting">
              <Save :size="16" />
              <span>{{ submitting ? 'Updating...' : 'Update Tags' }}</span>
            </button>
          </div>
        </div>
      </div>
    </div>

    <!-- Share Dialog -->
    <ShareDialog
      v-if="showShareModal && entryToShare"
      :storage="selectedStorage!"
      :entry="entryToShare"
      @close="showShareModal = false"
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

.header-actions {
  display: flex;
  gap: 12px;
}

.btn-secondary {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  color: var(--text-primary);
  font-weight: 500;
  cursor: pointer;
  transition: all 0.2s;
  white-space: nowrap;
}

.btn-secondary:hover {
  background: rgba(255, 255, 255, 0.1);
  border-color: var(--text-secondary);
}

/* Filter bar */
.filter-bar {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 8px 16px;
}

.filter-input-wrapper {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 180px;
}

.filter-input-icon {
  color: var(--text-secondary);
  flex-shrink: 0;
}

.filter-input {
  background: none;
  border: none;
  color: var(--text-primary);
  outline: none;
  font-size: 0.8125rem;
  width: 100%;
}

.filter-input::placeholder {
  color: var(--text-secondary);
  opacity: 0.6;
}

.filter-tags {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
  overflow: hidden;
}

.filter-tag {
  font-size: 0.65rem;
  padding: 2px 8px;
  background: rgba(255, 255, 255, 0.05);
  color: var(--text-secondary);
  border-radius: 4px;
  border: 1px solid var(--border-color);
  cursor: pointer;
  white-space: nowrap;
  transition: all 0.2s;
  user-select: none;
}

.filter-tag:hover {
  background: rgba(59, 130, 246, 0.08);
  color: var(--text-primary);
}

.filter-tag.active {
  background: rgba(59, 130, 246, 0.15);
  color: var(--accent-color);
  border-color: rgba(59, 130, 246, 0.3);
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
  grid-template-columns: 1fr 60px 100px 170px;
  padding: 8px 16px;
  background: rgba(255, 255, 255, 0.02);
  border-bottom: 1px solid var(--border-color);
  font-weight: 600;
  font-size: 0.75rem;
  color: var(--text-secondary);
  min-width: 0;
  gap: 16px;
}

@media (max-width: 1100px) {
  .table-header {
    grid-template-columns: 1fr 60px 170px;
  }
  .col-time { display: none; }
}

@media (max-width: 750px) {
  .table-header {
    grid-template-columns: 1fr 170px;
  }
  .col-size { display: none; }
}

.entries-list {
  overflow-y: auto;
  flex: 1;
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.entries-list::-webkit-scrollbar {
  display: none;
}

.entry-row {
  display: grid;
  grid-template-columns: 1fr 60px 100px 170px;
  padding: 4px 16px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.03);
  align-items: center;
  transition: background 0.2s;
  cursor: pointer;
  font-size: 0.8125rem;
  min-width: 0;
  gap: 16px;
}

@media (max-width: 1100px) {
  .entry-row {
    grid-template-columns: 1fr 60px 170px;
  }
}

@media (max-width: 750px) {
  .entry-row {
    grid-template-columns: 1fr 170px;
  }
}

.entry-row:hover {
  background: rgba(255, 255, 255, 0.03);
}

.col-name {
  display: flex;
  align-items: center;
  gap: 12px;
  overflow: hidden;
  min-width: 0;
}

.entry-name {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  font-weight: 500;
  flex: 0 1 auto;
  min-width: 0;
}

.entry-tags {
  display: flex;
  gap: 4px;
  overflow: hidden;
  mask-image: linear-gradient(to right, black 85%, transparent 100%);
  flex: 1;
  min-width: 0;
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

.entry-icon.folder { color: #f59e0b; }
.entry-icon.symlink { color: #06b6d4; }
.entry-icon.file { color: var(--text-secondary); }

.entry-thumbnail-container {
  width: 18px;
  height: 18px;
  display: flex;
  align-items: center;
  justify-content: center;
  position: relative;
}

.entry-thumbnail {
  width: 100%;
  height: 100%;
  object-fit: cover;
  border-radius: 2px;
  z-index: 2;
}

.fallback-icon {
  position: absolute;
  z-index: 1;
}

.col-size, .col-time {
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.75rem;
  color: var(--text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  text-align: right;
}

.action-buttons {
  display: flex;
  gap: 4px;
  opacity: 0;
  transition: opacity 0.2s;
  justify-content: flex-end;
  visibility: hidden;
}

.entry-row:hover .action-buttons {
  opacity: 1;
  visibility: visible;
}

@media (max-width: 750px) {
  .action-buttons {
    opacity: 1;
    visibility: visible;
  }
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

/* Modal styles */
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
  max-width: 400px;
  animation: slideUp 0.15s ease-out;
}

@keyframes slideUp {
  from { opacity: 0; transform: translateY(20px); }
  to { opacity: 1; transform: translateY(0); }
}

.explorer-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 0 4px;
  gap: 8px;
  border-bottom: 1px solid var(--border-color);
}

.modal-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 20px 24px;
}

.modal-header h3 {
  font-size: 1.125rem;
  margin: 0;
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

.form-group input {
  padding: 10px 12px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  color: var(--text-primary);
  outline: none;
  transition: all 0.2s;
}

.form-group input:focus {
  border-color: var(--accent-color);
  background: rgba(255, 255, 255, 0.08);
}

.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: 12px;
  padding-top: 8px;
}

/* Tags selection styles */
.modal-subtitle {
  font-size: 0.875rem;
  color: var(--text-secondary);
  margin-bottom: 8px;
}

.tags-selection-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
  gap: 10px;
  max-height: 300px;
  overflow-y: auto;
  padding: 4px;
}

.tag-checkbox-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 12px;
  cursor: pointer;
  transition: all 0.2s;
  border: 1px solid transparent;
}

.tag-checkbox-item:hover {
  background: rgba(255, 255, 255, 0.08);
}

.tag-checkbox-item.selected {
  background: rgba(59, 130, 246, 0.1);
  border-color: rgba(59, 130, 246, 0.3);
  color: var(--accent-color);
}

.hidden-checkbox {
  display: none;
}

.empty-tags-hint {
  grid-column: 1 / -1;
  text-align: center;
  padding: 20px;
  color: var(--text-secondary);
  font-size: 0.875rem;
}
.fade-in {
  animation: fadeIn 0.2s ease-out;
}

@keyframes fadeIn {
  from { opacity: 0; transform: scale(0.95); }
  to { opacity: 1; transform: scale(1); }
}

.tags-selection-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
  gap: 12px;
  max-height: 320px;
  overflow-y: auto;
  padding: 4px;
  margin-bottom: 24px;
}

.tag-checkbox-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px;
  cursor: pointer;
  transition: all 0.2s;
  border: 1px solid transparent;
}

.tag-checkbox-item:hover {
  background: rgba(255, 255, 255, 0.08);
}

.tag-checkbox-item.selected {
  background: rgba(59, 130, 246, 0.12);
  border-color: rgba(59, 130, 246, 0.4);
  color: var(--accent-color);
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
}

.header-title {
  display: flex;
  align-items: center;
  gap: 12px;
}

.header-icon {
  color: var(--accent-color);
}

.modal-subtitle {
  font-size: 0.875rem;
  color: var(--text-secondary);
  margin-bottom: 20px;
}

.hint {
  font-size: 0.75rem;
  opacity: 0.7;
  display: block;
}
</style>
