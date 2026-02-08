<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { 
  Share2, 
  Trash2, 
  Globe, 
  Users as GroupsIcon, 
  Clock
} from 'lucide-vue-next'
import { storageService } from '../services/storage'
import type { Shared } from '../types'
import GlobalShareEditDialog from './GlobalShareEditDialog.vue'

const shares = ref<Shared[]>([])
const loading = ref(true)
const error = ref<string | null>(null)
const selectedShareForEdit = ref<Shared | null>(null)
const showEditDialog = ref(false)

const fetchShares = async () => {
  try {
    loading.value = true
    error.value = null
    shares.value = await storageService.getAllShares()
  } catch (err: any) {
    error.value = err.message || 'Failed to fetch shares'
  } finally {
    loading.value = false
  }
}

const revokeShare = async (shareId: number) => {
  if (!confirm('Are you sure you want to revoke this share?')) return
  
  try {
    await storageService.deleteShare(shareId)
    shares.value = shares.value.filter(s => s.id !== shareId)
  } catch (err: any) {
    alert('Failed to revoke share: ' + err.message)
  }
}

const formatDate = (dateStr?: string) => {
  if (!dateStr) return 'Never'
  return new Date(dateStr).toLocaleString()
}

const isExpired = (dateStr?: string) => {
  if (!dateStr) return false
  return new Date(dateStr) < new Date()
}

const openEditDialog = (share: Shared) => {
  selectedShareForEdit.value = share
  showEditDialog.value = true
}

const handleUpdated = () => {
  fetchShares()
}


onMounted(fetchShares)
</script>

<template>
  <div class="share-management">
    <div class="area-header">
      <div class="header-content">
        <h2>My Shares</h2>
        <p>Manage all files and folders you have shared with others.</p>
      </div>
      <button class="btn-primary" @click="fetchShares">
        Refresh
      </button>
    </div>

    <div v-if="loading" class="loading-state">
      <div class="spinner"></div>
      <p>Loading shares...</p>
    </div>

    <div v-else-if="error" class="error-state glass-panel">
      <p>Error: {{ error }}</p>
      <button @click="fetchShares" class="btn-primary">Retry</button>
    </div>

    <div v-else-if="shares.length === 0" class="empty-state glass-panel fade-in">
      <Share2 :size="48" style="opacity: 0.3" />
      <p>You haven't shared any files yet.</p>
      <span class="tip">Go to the Files section to start sharing.</span>
    </div>

    <div v-else class="shares-list fade-in">
      <div v-for="share in shares" :key="share.id" class="share-card glass-panel" :class="{ expired: isExpired(share.expires_at) }">
        <div class="share-main">
          <div class="item-info">
            <div class="item-icon">
              <!-- Simple heuristic: folders often end with / or have no extension, but entries don't tell us easily here -->
              <Share2 :size="24" class="accent-icon" />
            </div>
            <div class="item-details">
              <span class="item-path">{{ share.path }}</span>
              <div class="item-meta">
                <span class="storage-badge">Storage #{{ share.storage_id }}</span>
                <span v-if="share.token" class="share-type">
                  <Globe :size="14" /> Public Link
                </span>
                <span v-if="share.user_ids?.length || share.group_ids?.length" class="share-type">
                  <GroupsIcon :size="14" /> Restricted
                </span>
              </div>
            </div>
          </div>

          <div class="share-options">
            <div class="option-stat">
              <Clock :size="14" />
              <span>Expires: {{ formatDate(share.expires_at) }}</span>
            </div>
            <div class="option-stat">
              <span class="permission-tag" :class="{ 'can-write': share.can_write }">
                {{ share.can_write ? 'Write Access' : 'Read Only' }}
              </span>
            </div>
          </div>
        </div>

        <div class="share-actions">
          <button class="btn-icon" @click="openEditDialog(share)" title="Edit Share">
            <Share2 :size="18" />
          </button>
          <button class="btn-icon danger" @click="revokeShare(share.id)" title="Revoke Share">
            <Trash2 :size="18" />
          </button>
        </div>
      </div>
    </div>

    <!-- Edit Dialog -->
    <GlobalShareEditDialog 
      v-if="showEditDialog && selectedShareForEdit" 
      :share="selectedShareForEdit" 
      @close="showEditDialog = false"
      @updated="handleUpdated"
    />
  </div>
</template>

<style scoped>
.share-management {
  display: flex;
  flex-direction: column;
  gap: 24px;
}

.shares-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.share-card {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px;
  transition: all 0.2s;
}

.share-card:hover {
  border-color: var(--accent-color);
  transform: translateX(4px);
}

.share-card.expired {
  opacity: 0.6;
}

.share-main {
  display: flex;
  align-items: center;
  gap: 40px;
  flex: 1;
}

.item-info {
  display: flex;
  align-items: center;
  gap: 16px;
  min-width: 300px;
}

.item-icon {
  width: 44px;
  height: 44px;
  background: rgba(59, 130, 246, 0.1);
  border-radius: 12px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.accent-icon {
  color: var(--accent-color);
}

.item-details {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.item-path {
  font-weight: 500;
  color: var(--text-primary);
  font-size: 0.95rem;
}

.item-meta {
  display: flex;
  align-items: center;
  gap: 12px;
}

.storage-badge {
  font-size: 0.75rem;
  background: rgba(255, 255, 255, 0.05);
  padding: 2px 8px;
  border-radius: 4px;
  color: var(--text-secondary);
}

.share-type {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 0.75rem;
  color: var(--accent-color);
}

.share-options {
  display: flex;
  align-items: center;
  gap: 24px;
}

.option-stat {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--text-secondary);
  font-size: 0.85rem;
}

.permission-tag {
  font-size: 0.75rem;
  padding: 4px 10px;
  background: rgba(255, 255, 255, 0.05);
  border-radius: 20px;
  color: var(--text-secondary);
}

.permission-tag.can-write {
  background: rgba(16, 185, 129, 0.1);
  color: var(--success-color);
  border: 1px solid rgba(16, 185, 129, 0.2);
}

.btn-icon {
  width: 36px;
  height: 36px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 8px;
  border: 1px solid var(--border-color);
  background: rgba(255, 255, 255, 0.03);
  color: var(--text-secondary);
  cursor: pointer;
  transition: all 0.2s;
}

.btn-icon:hover {
  background: rgba(255, 255, 255, 0.08);
  color: var(--text-primary);
}

.btn-icon.danger:hover {
  background: rgba(239, 68, 68, 0.1);
  color: var(--danger-color);
  border-color: rgba(239, 68, 68, 0.2);
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
</style>
