<script setup lang="ts">
import { ref } from 'vue'
import { 
  X, 
  Share2, 
  Globe, 
  UserPlus,
  Users as GroupsIcon
} from 'lucide-vue-next'
import { storageService } from '../services/storage'
import type { Shared } from '../types'
import UserSelect from './UserSelect.vue'
import GroupSelect from './GroupSelect.vue'

const props = defineProps<{
  share: Shared
}>()

const emit = defineEmits(['close', 'updated'])

const submitting = ref(false)
const canWrite = ref(props.share.can_write)
const expiresInDays = ref(0) // Default to no change in expiration or set 0
const selectedUserIds = ref<number[]>([...(props.share.user_ids || [])])
const selectedGroupIds = ref<number[]>([...(props.share.group_ids || [])])
const isPublicLink = ref(!!props.share.token)

const handleUpdate = async () => {
  try {
    submitting.value = true
    const payload = {
      can_write: canWrite.value,
      expires_in_days: expiresInDays.value > 0 ? expiresInDays.value : undefined,
      public_link: isPublicLink.value,
      user_ids: selectedUserIds.value,
      group_ids: selectedGroupIds.value
    }

    await storageService.updateShare(props.share.id, payload)
    emit('updated')
    emit('close')
  } catch (err: any) {
    alert('Failed to update share: ' + err.message)
  } finally {
    submitting.value = false
  }
}
</script>

<template>
  <div class="modal-overlay" @click.self="emit('close')">
    <div class="modal glass-panel fade-in">
      <div class="modal-header">
        <div class="header-title">
          <Share2 :size="20" class="header-icon" />
          <h3>Edit Share</h3>
        </div>
        <button class="btn-icon" @click="emit('close')" :disabled="submitting">
          <X :size="20" />
        </button>
      </div>
      
      <div class="modal-body">
        <p class="modal-subtitle">Sharing: <strong>{{ share.path }}</strong></p>
        
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
            @click="handleUpdate" 
            :disabled="submitting || (!isPublicLink && selectedUserIds.length === 0 && selectedGroupIds.length === 0)"
          >
            <Share2 :size="18" />
            <span>{{ submitting ? 'Updating...' : 'Update share' }}</span>
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

.btn-icon {
  background: none;
  border: none;
  color: var(--text-secondary);
  cursor: pointer;
  padding: 4px;
  transition: color 0.2s;
}

.btn-icon:hover {
  color: var(--text-primary);
}

.fade-in {
  animation: fadeIn 0.3s ease-out;
}

@keyframes fadeIn {
  from { opacity: 0; transform: translateY(-10px); }
  to { opacity: 1; transform: translateY(0); }
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
</style>
