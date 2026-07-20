<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { 
  X, 
  Share2, 
  Copy, 
  Trash2, 
  Clock, 
  Lock, 
  Globe, 
  UserPlus,
  Users as GroupsIcon
} from 'lucide-vue-next'
import { storageService } from '../services/storage'
import type { Storage, DirectoryEntry, Shared } from '../types'
import UserSelect from './UserSelect.vue'
import GroupSelect from './GroupSelect.vue'
import { userService } from '../services/user'
import { groupService } from '../services/group'
import type { User, Group } from '../types'

const props = defineProps<{
  storage: Storage
  entry: DirectoryEntry
}>()

const emit = defineEmits(['close'])

const shares = ref<Shared[]>([])
const loading = ref(true)
const submitting = ref(false)
const canWrite = ref(false)
const expiresInDays = ref(7)
const selectedUserIds = ref<number[]>([])
const selectedGroupIds = ref<number[]>([])
const isPublicLink = ref(false)
const editingShareId = ref<number | null>(null)
const allUsers = ref<User[]>([])
const allGroups = ref<Group[]>([])

const fetchShares = async () => {
  try {
    loading.value = true
    const fetchedShares = await storageService.getShares(props.storage.id, props.entry.path)
    shares.value = fetchedShares
  } catch (err) {
    console.error('Failed to fetch shares:', err)
  } finally {
    loading.value = false
  }
}

const loadShareForEditing = (share: Shared) => {
  editingShareId.value = share.id
  isPublicLink.value = share.has_public_link
  selectedUserIds.value = share.user_ids || []
  selectedGroupIds.value = share.group_ids || []
  canWrite.value = share.can_write
  // Reset expiration to 0 (default) when loading for editing as we don't store "original duration"
  expiresInDays.value = 0 
}

const resetEditor = () => {
  editingShareId.value = null
  isPublicLink.value = false
  selectedUserIds.value = []
  selectedGroupIds.value = []
  canWrite.value = false
  expiresInDays.value = 7
}

const handleShareOrUpdate = async () => {
  try {
    submitting.value = true
    const payload = {
      can_write: canWrite.value,
      expires_in_days: expiresInDays.value > 0 ? expiresInDays.value : undefined,
      public_link: isPublicLink.value,
      user_ids: selectedUserIds.value,
      group_ids: selectedGroupIds.value
    }

    if (editingShareId.value) {
      await storageService.updateShare(editingShareId.value, payload)
    } else {
      await storageService.shareEntry(props.storage.id, props.entry.path, payload)
    }
    
    await fetchShares()
    
    // Reset state after successful share
    if (!editingShareId.value) {
      resetEditor()
    }
  } catch (err: any) {
    alert('Failed to save share: ' + err.message)
  } finally {
    submitting.value = false
  }
}

const deleteShare = async (shareId: number) => {
  try {
    await storageService.deleteShare(shareId)
    await fetchShares()
  } catch (err: any) {
    alert('Failed to delete share: ' + err.message)
  }
}

const getBasename = (path: string) => {
  return path.split('/').filter(s => s).pop() || path
}

const formatDate = (dateStr?: string) => {
  if (!dateStr) return 'Never'
  return new Date(dateStr).toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    year: 'numeric'
  })
}

const copyToClipboard = (text: string) => {
  navigator.clipboard.writeText(text)
  alert('Link copied to clipboard!')
}

const getPublicLink = (token: string) => {
  const base = window.location.origin
  return `${base}/api/storage/share/${token}/list`
}

const getShareTargetNames = (share: Shared) => {
  const names: string[] = []
  
  if (share.user_ids?.length) {
    share.user_ids.forEach(id => {
      const u = allUsers.value.find(u => u.id === id)
      if (u) names.push(u.name)
      else names.push(`User #${id}`)
    })
  }
  
  if (share.group_ids?.length) {
    share.group_ids.forEach(id => {
      const g = allGroups.value.find(g => g.id === id)
      if (g) names.push(`Group: ${g.name}`)
      else names.push(`Group #${id}`)
    })
  }

  return names.join(', ')
}

onMounted(async () => {
  fetchShares()
  // Fetch users and groups for name resolution
  try {
    const [u, g] = await Promise.all([
      userService.getUsers(),
      groupService.getGroups()
    ])
    allUsers.value = u
    allGroups.value = g
  } catch (err) {
    console.error('Failed to fetch metadata:', err)
  }
})
</script>

<template>
  <div class="modal-overlay" @click.self="emit('close')">
    <div class="modal glass-panel fade-in">
      <div class="modal-header">
        <div class="header-title">
          <Share2 :size="20" class="header-icon" />
          <h3>Share Entry</h3>
        </div>
        <button class="btn-icon" @click="emit('close')" :disabled="submitting">
          <X :size="20" />
        </button>
      </div>
      
      <div class="modal-body">
        <p class="modal-subtitle">Sharing: <strong>{{ getBasename(entry.path) }}</strong></p>
        
        <!-- Share Options -->
        <div class="share-options glass-panel">
          <div class="setting-row">
            <div class="setting-info">
              <label><Globe :size="16" style="display: inline; vertical-align: middle; margin-right: 4px;" /> Public Link</label>
              <span>Anyone with the link can access</span>
            </div>
            <label class="toggle">
              <input type="checkbox" v-model="isPublicLink" />
              <span class="slider"></span>
            </label>
          </div>

          <div class="selection-section">
            <label class="section-label"><UserPlus :size="16" /> Shared Users</label>
            <UserSelect v-model="selectedUserIds" multiple />
          </div>

          <div class="selection-section">
            <label class="section-label"><GroupsIcon :size="16" /> Shared Groups</label>
            <GroupSelect v-model="selectedGroupIds" multiple />
          </div>

          <div class="divider"></div>

          <div class="setting-row">
            <div class="setting-info">
              <label>Allow Write Access</label>
              <span>Recipients can edit or upload files</span>
            </div>
            <label class="toggle">
              <input type="checkbox" v-model="canWrite" />
              <span class="slider"></span>
            </label>
          </div>

          <div class="setting-row">
            <div class="setting-info">
              <label>Expiration (days)</label>
              <span>Set 0 for never</span>
            </div>
            <input 
              type="number" 
              v-model="expiresInDays" 
              class="number-input" 
              min="0"
            />
          </div>

          <button 
            class="btn-primary share-btn" 
            @click="handleShareOrUpdate" 
            :disabled="submitting || (!isPublicLink && selectedUserIds.length === 0 && selectedGroupIds.length === 0)"
          >
            <Share2 :size="18" />
            <span>{{ submitting ? (editingShareId ? 'Updating...' : 'Sharing...') : (editingShareId ? 'Update share' : 'Share now') }}</span>
          </button>
          
          <button 
            v-if="editingShareId" 
            class="btn-ghost btn-sm" 
            style="margin-top: 8px;"
            @click="resetEditor"
          >
            Create new share instead
          </button>
        </div>

        <!-- Existing Shares -->
        <div class="existing-shares">
          <h4>Current Shares</h4>
          
          <div v-if="loading" class="shares-loading">
            <div class="spinner-sm"></div>
          </div>
          
          <div v-else-if="shares.length === 0" class="empty-state">
            <p>No active shares for this entry.</p>
          </div>
          
          <div v-else class="shares-list">
            <div v-for="share in shares" :key="share.id" class="share-item glass-panel" :class="{ 'is-editing': editingShareId === share.id }">
              <div class="share-main">
                <div class="share-type-icon">
                  <Globe v-if="share.has_public_link" :size="16" />
                  <UserPlus v-else :size="16" />
                </div>
                <div class="share-details">
                  <span class="share-target">
                    {{ share.has_public_link ? 'Public Link' : getShareTargetNames(share) }}
                  </span>
                  <div class="share-meta">
                    <span class="meta-item">
                      <Clock :size="12" />
                      {{ formatDate(share.expires_at) }}
                    </span>
                    <span class="meta-item">
                      <Lock :size="12" />
                      {{ share.can_write ? 'Read/Write' : 'Read-only' }}
                    </span>
                    <span v-if="share.has_public_link && !share.token" class="meta-item public-link-hint">
                      Public link active — edit and re-enable to get a fresh copyable link
                    </span>
                  </div>
                </div>
              </div>
              
              <div class="share-actions">
                <button 
                  class="btn-icon-sm" 
                  @click="loadShareForEditing(share)"
                  title="Edit share"
                >
                  <Share2 :size="14" />
                </button>
                <a 
                  v-if="share.token" 
                  :href="getPublicLink(share.token)"
                  target="_blank"
                  class="btn-icon-sm"
                  title="Open link"
                  @click.stop
                >
                  <Globe :size="14" />
                </a>
                <button 
                  v-if="share.token" 
                  class="btn-icon-sm" 
                  @click="copyToClipboard(getPublicLink(share.token))"
                  title="Copy link"
                >
                  <Copy :size="14" />
                </button>
                <button 
                  class="btn-icon-sm danger" 
                  @click="deleteShare(share.id)"
                  title="Revoke share"
                >
                  <Trash2 :size="14" />
                </button>
              </div>
            </div>
          </div>
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
  max-width: 500px;
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
  max-height: 80vh;
  overflow-y: auto;
  scrollbar-width: thin;
}

.modal-subtitle {
  font-size: 0.875rem;
  color: var(--text-secondary);
  margin-bottom: 20px;
}

.share-options {
  padding: 24px;
  display: flex;
  flex-direction: column;
  gap: 20px;
  margin-bottom: 24px;
}

.selection-section {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.section-label {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 0.875rem;
  font-weight: 500;
  color: var(--text-primary);
}

.divider {
  height: 1px;
  background: var(--border-color);
  margin: 4px 0;
}

.selection-box {
  margin-bottom: 8px;
}

.setting-row {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 20px;
}

.setting-info {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.setting-info label {
  font-size: 0.875rem;
  font-weight: 500;
}

.setting-info span {
  font-size: 0.75rem;
  color: var(--text-secondary);
}

.number-input {
  width: 70px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  color: white;
  padding: 6px 10px;
  border-radius: 6px;
  outline: none;
}

.share-btn {
  margin-top: 8px;
  width: 100%;
}

/* Toggle Switch */
.toggle {
  position: relative;
  display: inline-block;
  width: 44px;
  height: 22px;
}

.toggle input {
  opacity: 0;
  width: 0;
  height: 0;
}

.slider {
  position: absolute;
  cursor: pointer;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background-color: rgba(255, 255, 255, 0.1);
  transition: .2s;
  border-radius: 34px;
}

.slider:before {
  position: absolute;
  content: "";
  height: 16px;
  width: 16px;
  left: 3px;
  bottom: 3px;
  background-color: white;
  transition: .2s;
  border-radius: 50%;
}

input:checked + .slider {
  background-color: var(--accent-color);
}

input:checked + .slider:before {
  transform: translateX(22px);
}

/* Existing Shares */
.existing-shares h4 {
  font-size: 1rem;
  margin-bottom: 12px;
  color: var(--text-primary);
}

.shares-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.share-item {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 12px;
  background: rgba(255, 255, 255, 0.02);
  transition: all 0.2s;
  border: 1px solid transparent;
}

.share-item.is-editing {
  border-color: var(--accent-color);
  background: rgba(59, 130, 246, 0.05);
}

.btn-ghost {
  background: none;
  border: 1px solid transparent;
  color: var(--text-secondary);
  cursor: pointer;
  padding: 4px 8px;
  border-radius: 4px;
  font-size: 0.75rem;
}

.btn-ghost:hover {
  color: var(--text-primary);
  background: rgba(255, 255, 255, 0.05);
}

.share-main {
  display: flex;
  align-items: center;
  gap: 12px;
}

.share-type-icon {
  width: 32px;
  height: 32px;
  border-radius: 8px;
  background: rgba(59, 130, 246, 0.1);
  color: var(--accent-color);
  display: flex;
  align-items: center;
  justify-content: center;
}

.share-details {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.share-target {
  font-size: 0.875rem;
  font-weight: 500;
}

.share-meta {
  display: flex;
  gap: 12px;
  font-size: 0.75rem;
  color: var(--text-secondary);
}

.meta-item {
  display: flex;
  align-items: center;
  gap: 4px;
}

.public-link-hint {
  color: var(--text-secondary);
  font-style: italic;
  flex-basis: 100%;
}

.share-actions {
  display: flex;
  gap: 8px;
}

.empty-state {
  text-align: center;
  padding: 30px;
  color: var(--text-secondary);
  background: rgba(255, 255, 255, 0.02);
  border-radius: 12px;
  border: 1px dashed var(--border-color);
}

.shares-loading {
  display: flex;
  justify-content: center;
  padding: 20px;
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
