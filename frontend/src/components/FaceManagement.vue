<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { ScanFace, RefreshCw } from 'lucide-vue-next'
import { faceService } from '../services/face'
import { useAuth } from '../composables/useAuth'
import { errorMessage } from '../utils/errors'
import FaceContactsPanel from './FaceContactsPanel.vue'
import FaceReviewQueue from './FaceReviewQueue.vue'
import type { Contact, RematchResult } from '../api'

const { user } = useAuth()
const isAdmin = computed(() => user.value?.admin === true)

const tab = ref<'contacts' | 'faces'>('contacts')

// Contacts are shared by both panels — one source of truth here so a rename
// in one tab is instantly reflected in the other's dropdowns.
const contacts = ref<Contact[]>([])
const loading = ref(false)
const error = ref<string | null>(null)

// Bumped after any label mutation so the review queue refetches its page.
const refreshKey = ref(0)

const rematching = ref(false)
const rematchSummary = ref<string | null>(null)

const describeRematch = (r: RematchResult) =>
  `considered ${r.considered} · assigned ${r.assigned} · cleared ${r.cleared}` +
  ` · unchanged ${r.unchanged} · skipped ${r.skipped} · metas updated ${r.metas_updated}`

const fetchContacts = async () => {
  try {
    loading.value = true
    error.value = null
    contacts.value = await faceService.getContacts()
  } catch (err: unknown) {
    error.value = 'Failed to load contacts: ' + errorMessage(err)
  } finally {
    loading.value = false
  }
}

const onLabelsChanged = () => {
  refreshKey.value++
  fetchContacts()
}

const runRematch = async () => {
  try {
    rematching.value = true
    rematchSummary.value = null
    const result = await faceService.rematch()
    rematchSummary.value = describeRematch(result)
    refreshKey.value++
    await fetchContacts()
  } catch (err: unknown) {
    alert('Re-match failed: ' + errorMessage(err))
  } finally {
    rematching.value = false
  }
}

onMounted(fetchContacts)
</script>

<template>
  <div class="face-management fade-in">
    <div class="area-header">
      <div class="header-content">
        <h2><ScanFace :size="24" class="title-icon" /> Face Management</h2>
        <p>
          Named contacts and the face review queue.
          {{ isAdmin ? 'Confirm exemplars and the matcher labels the rest.' : 'Read-only view — labeling requires an admin.' }}
        </p>
      </div>
      <button
        v-if="isAdmin"
        class="btn-primary rematch-btn"
        :disabled="rematching"
        title="Re-run automatic matching over suggested faces"
        @click="runRematch"
      >
        <RefreshCw :size="18" :class="{ spinning: rematching }" />
        <span>{{ rematching ? 'Matching…' : 'Re-run matching' }}</span>
      </button>
    </div>

    <p v-if="rematchSummary" class="rematch-summary glass-panel">
      Re-match finished — {{ rematchSummary }}
    </p>

    <div class="tabs">
      <button :class="['tab', { active: tab === 'contacts' }]" @click="tab = 'contacts'">
        Contacts
      </button>
      <button :class="['tab', { active: tab === 'faces' }]" @click="tab = 'faces'">
        Faces &amp; Review Queue
      </button>
    </div>

    <div v-if="loading && contacts.length === 0" class="loading-state">
      <div class="spinner"></div>
      <p>Loading face data…</p>
    </div>

    <div v-else-if="error" class="error-state glass-panel">
      <p>{{ error }}</p>
      <button class="btn-primary" @click="fetchContacts">Retry</button>
    </div>

    <template v-else>
      <FaceContactsPanel
        v-if="tab === 'contacts'"
        :contacts="contacts"
        :is-admin="isAdmin"
        @refresh="onLabelsChanged"
      />
      <FaceReviewQueue
        v-else
        :contacts="contacts"
        :is-admin="isAdmin"
        :refresh-key="refreshKey"
        @refresh="onLabelsChanged"
      />
    </template>
  </div>
</template>

<style scoped>
.face-management {
  display: flex;
  flex-direction: column;
  gap: 24px;
}

.area-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 16px;
}

.area-header h2 {
  display: flex;
  align-items: center;
  gap: 10px;
  font-size: 1.75rem;
  margin-bottom: 4px;
}

.title-icon {
  color: var(--accent-color);
}

.area-header p {
  color: var(--text-secondary);
}

/* Icon + label inside the global `.btn-primary`. */
.rematch-btn {
  display: flex;
  align-items: center;
  gap: 8px;
}

.rematch-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.spinning {
  animation: spin 1s linear infinite;
}

.rematch-summary {
  padding: 12px 16px;
  font-size: 0.875rem;
  color: var(--success-color);
  background: rgba(16, 185, 129, 0.08);
  border: 1px solid rgba(16, 185, 129, 0.2);
}

.tabs {
  display: flex;
  gap: 4px;
  border-bottom: 1px solid var(--border-color);
}

.tab {
  padding: 12px 20px;
  color: var(--text-secondary);
  font-weight: 500;
  border-bottom: 2px solid transparent;
  border-radius: 8px 8px 0 0;
}

.tab:hover {
  color: var(--text-primary);
}

.tab.active {
  color: var(--accent-color);
  border-bottom-color: var(--accent-color);
}

.loading-state,
.error-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 80px;
  text-align: center;
  gap: 16px;
}

.error-state {
  color: var(--danger-color);
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
</style>
