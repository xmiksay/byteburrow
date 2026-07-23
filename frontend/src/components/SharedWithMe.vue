<script setup lang="ts">
import { ref, onMounted, computed, watch } from 'vue'
import { 
  Folder, 
  File, 
  ChevronRight, 
  ArrowLeft, 
  Download, 
  Eye, 
  Share2,
  Link,
  Lock,
  Unlock,
  FolderPlus,
  FilePlus,
  Edit2,
  Trash2,
  X,
  Save,
  Tag as TagIcon,
  Search
} from 'lucide-vue-next'
import { useRoute, useRouter } from 'vue-router'
import { storageService } from '../services/storage'
import { tagService } from '../services/tag'
import type { DirectoryEntry, Shared, Storage, Tag } from '../types'
import { formatSize, formatDate, getBasename, isViewableAsText, isImage, isMediaFile, getThumbnailUrl, sortEntries } from '../utils/file'
import MediaViewer from './MediaViewer.vue'
import FileViewer from './FileViewer.vue'
import TagDialog from './TagDialog.vue'

const route = useRoute()
const router = useRouter()

const shares = ref<Shared[]>([])
const selectedShare = ref<Shared | null>(null)
const currentPath = ref<string>('')
const entries = ref<DirectoryEntry[]>([])
const loading = ref(false)
const error = ref<string | null>(null)
const selectedFile = ref<DirectoryEntry | null>(null)
const selectedMediaFile = ref<DirectoryEntry | null>(null)
const entryForTags = ref<DirectoryEntry | null>(null)
const showTagsModal = ref(false)

// Write operation state
const showCreateModal = ref(false)
const createType = ref<'Directory' | 'File'>('Directory')
const newEntryName = ref('')
const creating = ref(false)

const entryToRename = ref<DirectoryEntry | null>(null)
const newName = ref('')
const renaming = ref(false)

const entryToDelete = ref<DirectoryEntry | null>(null)
const deleting = ref(false)

const canWrite = computed(() => selectedShare.value?.can_write ?? false)

// Create a mock storage object for FileViewer
const mockStorage = computed<Storage>(() => ({
  id: selectedShare.value?.storage_id ?? 0,
  name: '',
  path: '',
  default_user: 0,
  default_group: 0,
  ignore_patterns: ''
}))

const pathSegments = computed(() => {
  if (!currentPath.value) return []
  return currentPath.value.split('/').filter(s => s)
})

const fetchShares = async () => {
  try {
    loading.value = true
    error.value = null
    shares.value = await storageService.getSharesWithMe()

    const routeShareId = route.params.shareId ? Number(route.params.shareId) : undefined
    const routePath = (route.params.path as string) || ''

    if (routeShareId) {
      const found = shares.value.find(s => s.id === routeShareId)
      if (found) {
        selectedShare.value = found
        currentPath.value = routePath
        fetchEntries()
        return
      }
    }

    if (shares.value.length > 0) {
      router.replace({ name: 'shared', params: { shareId: shares.value[0].id, path: '' } })
    }
  } catch (err: any) {
    error.value = 'Failed to load shares: ' + err.message
  } finally {
    loading.value = false
  }
}

const selectShare = (share: Shared) => {
  router.push({ name: 'shared', params: { shareId: share.id, path: '' } })
}

const onShareSelect = (event: Event) => {
  const id = Number((event.target as HTMLSelectElement).value)
  const share = shares.value.find(s => s.id === id)
  if (share) selectShare(share)
}

const fetchEntries = async () => {
  if (!selectedShare.value) return
  
  try {
    loading.value = true
    error.value = null
    const response = await storageService.listShareDirectory(selectedShare.value.id, currentPath.value)
    entries.value = response.entries
  } catch (err: any) {
    error.value = 'Failed to load directory: ' + err.message
  } finally {
    loading.value = false
  }
}

const navigateTo = (path: string) => {
  if (!selectedShare.value) return
  router.push({ name: 'shared', params: { shareId: selectedShare.value.id, path } })
}

const navigateUp = () => {
  const segments = pathSegments.value
  if (segments.length === 0) return
  const parentPath = segments.slice(0, -1).join('/')
  navigateTo(parentPath)
}

// React to route changes (back/forward navigation, programmatic pushes)
watch(
  () => [route.params.shareId, route.params.path],
  ([newShareId, newPath]) => {
    if (route.name !== 'shared') return
    const id = newShareId ? Number(newShareId) : undefined
    const path = (newPath as string) || ''

    if (id && id !== selectedShare.value?.id) {
      const found = shares.value.find(s => s.id === id)
      if (found) {
        selectedShare.value = found
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
  if (!selectedShare.value) return '#'
  return `/api/storage/share/${selectedShare.value.id}/${mode}/${entry.path}`
}

const sortedEntries = computed(() => {
  let result = entries.value
  const nameQuery = filterName.value.trim().toLowerCase()
  if (nameQuery) {
    result = result.filter(e => getBasename(e.path).toLowerCase().includes(nameQuery))
  }
  if (filterTags.value.length > 0) {
    result = result.filter(e => filterTags.value.every(t => (e.tags ?? []).includes(t)))
  }
  return sortEntries(result)
})

const getShareLabel = (share: Shared) => {
  const basename = share.path ? getBasename(share.path) : 'Root'
  return `${basename} (Storage #${share.storage_id})`
}

const handleEntryClick = (entry: DirectoryEntry) => {
  if (entry.entry_type === 'Directory' || (entry.entry_type === 'Symlink' && !isMediaFile(entry.path) && !isViewableAsText(entry.path))) {
    navigateTo(entry.path)
  } else if (isViewableAsText(entry.path)) {
    openFileViewer(entry)
  } else if (isMediaFile(entry.path)) {
    selectedMediaFile.value = entry
  }
}

const openFileViewer = (entry: DirectoryEntry) => {
  selectedFile.value = entry
}

const closeFileViewer = () => {
  selectedFile.value = null
}

// Write operations
const openCreateModal = (type: 'Directory' | 'File') => {
  createType.value = type
  newEntryName.value = ''
  showCreateModal.value = true
}

const handleCreate = async () => {
  if (!selectedShare.value || !newEntryName.value.trim()) return
  
  try {
    creating.value = true
    const path = currentPath.value 
      ? `${currentPath.value}/${newEntryName.value.trim()}`
      : newEntryName.value.trim()
    await storageService.createShareEntry(selectedShare.value.id, path, createType.value)
    showCreateModal.value = false
    newEntryName.value = ''
    await fetchEntries()
  } catch (err: any) {
    alert('Failed to create: ' + err.message)
  } finally {
    creating.value = false
  }
}

const startRename = (entry: DirectoryEntry) => {
  entryToRename.value = entry
  newName.value = getBasename(entry.path)
}

const cancelRename = () => {
  entryToRename.value = null
  newName.value = ''
}

const handleRename = async () => {
  if (!selectedShare.value || !entryToRename.value || !newName.value.trim()) return
  
  try {
    renaming.value = true
    const parentPath = entryToRename.value.path.split('/').slice(0, -1).join('/')
    const newPath = parentPath ? `${parentPath}/${newName.value.trim()}` : newName.value.trim()
    await storageService.renameShareEntry(selectedShare.value.id, entryToRename.value.path, newPath)
    entryToRename.value = null
    newName.value = ''
    await fetchEntries()
  } catch (err: any) {
    alert('Failed to rename: ' + err.message)
  } finally {
    renaming.value = false
  }
}

const confirmDelete = (entry: DirectoryEntry) => {
  entryToDelete.value = entry
}

const handleDelete = async () => {
  if (!selectedShare.value || !entryToDelete.value) return
  
  try {
    deleting.value = true
    await storageService.removeShareEntry(selectedShare.value.id, entryToDelete.value.path)
    entryToDelete.value = null
    await fetchEntries()
  } catch (err: any) {
    alert('Failed to delete: ' + err.message)
  } finally {
    deleting.value = false
  }
}

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

// Tags logic
const allTags = ref<Tag[]>([])

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

onMounted(() => {
  fetchShares()
  fetchTags()
})
</script>

<template>
  <div class="shared-explorer">
    <div class="explorer-header">
      <div class="share-selector glass-panel">
        <Share2 :size="18" />
        <select :value="selectedShare?.id" @change="onShareSelect($event)" class="share-select">
          <option v-for="s in shares" :key="s.id" :value="s.id">{{ getShareLabel(s) }}</option>
        </select>
        <div v-if="selectedShare" class="share-meta">
          <span v-if="selectedShare.can_write" class="permission write" title="Write Access">
            <Unlock :size="14" />
          </span>
          <span v-else class="permission read" title="Read Only">
            <Lock :size="14" />
          </span>
        </div>
      </div>

      <div class="breadcrumb-container glass-panel">
        <button class="btn-icon" @click="navigateTo('')" title="Share Root">
          <Share2 :size="18" />
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

      <!-- Write actions -->
      <div v-if="canWrite" class="write-actions">
        <button class="btn-icon glass-panel" @click="openCreateModal('Directory')" title="New Folder">
          <FolderPlus :size="18" />
        </button>
        <button class="btn-icon glass-panel" @click="openCreateModal('File')" title="New File">
          <FilePlus :size="18" />
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

    <div v-if="shares.length === 0 && !loading" class="empty-state glass-panel fade-in">
      <Share2 :size="48" style="opacity: 0.3" />
      <p>No files have been shared with you yet.</p>
      <span class="tip">When someone shares a file or folder with you, it will appear here.</span>
    </div>

    <div v-else-if="error" class="error-state glass-panel fade-in">
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
            
            <!-- Inline rename -->
            <template v-if="entryToRename?.path === entry.path">
              <input 
                v-model="newName" 
                class="rename-input" 
                @click.stop 
                @keyup.enter="handleRename"
                @keyup.escape="cancelRename"
                autofocus
              />
              <button class="btn-icon-sm" @click.stop="handleRename" :disabled="renaming">
                <Save :size="14" />
              </button>
              <button class="btn-icon-sm" @click.stop="cancelRename">
                <X :size="14" />
              </button>
            </template>
            <span v-else class="entry-name">{{ getBasename(entry.path) }}</span>
            
            <div class="entry-tags" v-if="entry.tags?.length">
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
              <template v-if="canWrite">
                <button class="btn-icon-sm" @click.stop="startRename(entry)" title="Rename">
                  <Edit2 :size="16" />
                </button>
                <button class="btn-icon-sm" @click.stop="openTagsModal(entry)" title="Manage Tags">
                  <TagIcon :size="16" />
                </button>
                <button class="btn-icon-sm danger" @click.stop="confirmDelete(entry)" title="Delete">
                  <Trash2 :size="16" />
                </button>
              </template>
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
      v-if="selectedFile && selectedShare"
      :storage="mockStorage"
      :entry="selectedFile"
      :share-id="selectedShare.id"
      :read-only="!canWrite"
      @close="closeFileViewer"
    />

    <!-- Media Viewer Modal -->
    <MediaViewer
      v-if="selectedMediaFile && selectedShare"
      :storage="mockStorage"
      :entry="selectedMediaFile"
      :share-id="selectedShare.id"
      @close="selectedMediaFile = null"
    />

    <!-- Tags Dialog -->
    <TagDialog
      v-if="showTagsModal && entryForTags && selectedShare"
      :storage="mockStorage"
      :entry="entryForTags"
      :share-id="selectedShare.id"
      @close="showTagsModal = false"
      @updated="handleTagsUpdated"
    />

    <!-- Create Modal -->
    <div v-if="showCreateModal" class="modal-overlay" @click.self="showCreateModal = false">
      <div class="modal glass-panel">
        <div class="modal-header">
          <h3>Create {{ createType === 'Directory' ? 'Folder' : 'File' }}</h3>
          <button class="btn-icon close-btn" @click="showCreateModal = false">×</button>
        </div>
        <div class="modal-body">
          <label class="form-label">Name</label>
          <input 
            v-model="newEntryName" 
            class="form-input" 
            :placeholder="createType === 'Directory' ? 'New folder' : 'filename.txt'"
            @keyup.enter="handleCreate"
          />
        </div>
        <div class="modal-footer">
          <button class="btn-secondary" @click="showCreateModal = false">Cancel</button>
          <button class="btn-primary" @click="handleCreate" :disabled="creating || !newEntryName.trim()">
            {{ creating ? 'Creating...' : 'Create' }}
          </button>
        </div>
      </div>
    </div>

    <!-- Delete Confirmation Modal -->
    <div v-if="entryToDelete" class="modal-overlay" @click.self="entryToDelete = null">
      <div class="modal glass-panel">
        <div class="modal-header">
          <h3>Confirm Delete</h3>
          <button class="btn-icon close-btn" @click="entryToDelete = null">×</button>
        </div>
        <div class="modal-body">
          <p>Are you sure you want to delete <strong>{{ getBasename(entryToDelete.path) }}</strong>?</p>
          <p v-if="entryToDelete.entry_type === 'Directory'" class="warning-text">
            This will delete all contents inside the folder.
          </p>
        </div>
        <div class="modal-footer">
          <button class="btn-secondary" @click="entryToDelete = null">Cancel</button>
          <button class="btn-danger" @click="handleDelete" :disabled="deleting">
            {{ deleting ? 'Deleting...' : 'Delete' }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.shared-explorer {
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

.share-selector {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 16px;
  min-width: 280px;
}

.share-select {
  background: none;
  border: none;
  color: var(--text-primary);
  font-weight: 500;
  outline: none;
  flex: 1;
  cursor: pointer;
}

.share-select option {
  background: #1a1a1a;
  color: white;
}

.share-meta {
  display: flex;
  align-items: center;
  gap: 8px;
}

.permission {
  display: flex;
  align-items: center;
  padding: 4px;
  border-radius: 4px;
}

.permission.write {
  color: var(--success-color);
  background: rgba(16, 185, 129, 0.1);
}

.permission.read {
  color: var(--text-secondary);
  background: rgba(255, 255, 255, 0.05);
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

.write-actions {
  display: flex;
  gap: 8px;
}

.write-actions .btn-icon {
  padding: 8px;
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
  grid-template-columns: 1fr 60px 100px 140px;
  padding: 8px 16px;
  background: rgba(255, 255, 255, 0.02);
  border-bottom: 1px solid var(--border-color);
  font-weight: 600;
  font-size: 0.75rem;
  color: var(--text-secondary);
  min-width: 0;
  gap: 16px;
}

.entries-list {
  overflow-y: auto;
  flex: 1;
  scrollbar-width: none;
}

.entries-list::-webkit-scrollbar {
  display: none;
}

.entry-row {
  display: grid;
  grid-template-columns: 1fr 60px 100px 140px;
  padding: 4px 16px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.03);
  align-items: center;
  transition: background 0.2s;
  cursor: pointer;
  font-size: 0.8125rem;
  min-width: 0;
  gap: 16px;
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

.col-size, .col-date {
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

.btn-icon-sm {
  padding: 6px;
  border-radius: 6px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  color: var(--text-secondary);
  cursor: pointer;
  transition: all 0.2s;
  display: flex;
  align-items: center;
  justify-content: center;
  text-decoration: none;
}

.btn-icon-sm:hover {
  background: rgba(255, 255, 255, 0.1);
  color: var(--text-primary);
}

.btn-icon-sm.danger:hover {
  background: rgba(239, 68, 68, 0.1);
  color: #ef4444;
  border-color: rgba(239, 68, 68, 0.3);
}

.btn-icon {
  padding: 6px;
  border-radius: 6px;
  background: transparent;
  border: none;
  color: var(--text-secondary);
  cursor: pointer;
  transition: all 0.2s;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 1.25rem;
}

.btn-icon:hover {
  color: var(--text-primary);
}

.rename-input {
  flex: 1;
  background: rgba(0, 0, 0, 0.3);
  border: 1px solid var(--border-color);
  border-radius: 4px;
  padding: 4px 8px;
  color: var(--text-primary);
  font-size: 0.8125rem;
  outline: none;
  min-width: 100px;
}

.rename-input:focus {
  border-color: var(--accent-color);
}

.loading-overlay {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.5);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 10;
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

.empty-state, .error-state, .empty-directory {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 80px;
  text-align: center;
  gap: 16px;
}

.tip {
  font-size: 0.85rem;
  color: var(--text-secondary);
}

.modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.7);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
}

.modal {
  width: 90%;
  max-width: 500px;
  display: flex;
  flex-direction: column;
}

.file-viewer-modal {
  max-width: 900px;
  max-height: 80vh;
  padding: 0;
}

.modal-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px;
  border-bottom: 1px solid var(--border-color);
}

.modal-header h3 {
  margin: 0;
  font-size: 1rem;
}

.close-btn {
  font-size: 1.5rem;
  line-height: 1;
}

.modal-body {
  padding: 20px;
  overflow: auto;
}

.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: 12px;
  padding: 16px 20px;
  border-top: 1px solid var(--border-color);
}

.form-label {
  display: block;
  margin-bottom: 8px;
  font-weight: 500;
  font-size: 0.85rem;
}

.form-input {
  width: 100%;
  padding: 10px 14px;
  background: rgba(0, 0, 0, 0.3);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  color: var(--text-primary);
  font-size: 0.9rem;
  outline: none;
}

.form-input:focus {
  border-color: var(--accent-color);
}

.file-loading, .file-error {
  padding: 20px;
  text-align: center;
}

.file-error {
  color: var(--danger-color);
}

.file-content {
  margin: 0;
  padding: 16px;
  background: rgba(0, 0, 0, 0.3);
  border-radius: 8px;
  overflow: auto;
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.85rem;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
  max-height: 60vh;
}

.warning-text {
  color: var(--warning-color);
  font-size: 0.85rem;
}

.btn-primary {
  padding: 10px 20px;
  background: var(--accent-color);
  border: none;
  border-radius: 8px;
  color: white;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.2s;
}

.btn-primary:hover:not(:disabled) {
  filter: brightness(1.1);
}

.btn-primary:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.btn-secondary {
  padding: 10px 20px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  color: var(--text-primary);
  font-weight: 500;
  cursor: pointer;
  transition: all 0.2s;
}

.btn-secondary:hover {
  background: rgba(255, 255, 255, 0.1);
}

.btn-danger {
  padding: 10px 20px;
  background: #ef4444;
  border: none;
  border-radius: 8px;
  color: white;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.2s;
}

.btn-danger:hover:not(:disabled) {
  filter: brightness(1.1);
}

.btn-danger:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.btn-sm {
  padding: 6px 12px;
  font-size: 0.8rem;
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.modal-header-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.file-editor {
  width: 100%;
  height: 60vh;
  padding: 16px;
  background: rgba(0, 0, 0, 0.3);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  color: var(--text-primary);
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.85rem;
  line-height: 1.5;
  resize: none;
  outline: none;
}

.file-editor:focus {
  border-color: var(--accent-color);
}
</style>
