<script setup lang="ts">
import { ref, reactive, onMounted, watch } from 'vue'
import { ScanFace, ChevronLeft, ChevronRight } from 'lucide-vue-next'
import { faceService } from '../services/face'
import { errorMessage } from '../utils/errors'
import FaceRefRow from './FaceRefRow.vue'
import type { Contact, FaceRef } from '../api'

const props = defineProps<{
  contacts: Contact[]
  isAdmin: boolean
  /** Bumped by the parent whenever labels may have changed elsewhere. */
  refreshKey: number
}>()

const emit = defineEmits<{
  (e: 'refresh'): void
}>()

const PER_PAGE = 50

const faces = ref<FaceRef[]>([])
const page = ref(1)
const totalPages = ref(1)
const total = ref(0)
const loading = ref(false)
const error = ref<string | null>(null)
const busyFaceId = ref<number | null>(null)

const filters = reactive({
  contactId: '' as '' | number,
  confirmed: 'all' as 'all' | 'yes' | 'no',
  unassigned: false,
})

const fetchFaces = async () => {
  try {
    loading.value = true
    error.value = null
    const result = await faceService.getRefs(
      {
        contact_id: filters.contactId === '' ? undefined : filters.contactId,
        confirmed: filters.confirmed === 'all' ? undefined : filters.confirmed === 'yes',
        unassigned: filters.unassigned || undefined,
      },
      page.value,
      PER_PAGE,
    )
    faces.value = result.items
    totalPages.value = result.total_pages
    total.value = result.total
  } catch (err: unknown) {
    error.value = 'Failed to load faces: ' + errorMessage(err)
  } finally {
    loading.value = false
  }
}

// Filter edits restart from the first page.
watch(
  () => ({ ...filters }),
  () => {
    page.value = 1
    fetchFaces()
  },
)

// Labels changed elsewhere (contact deleted, rematch ran) — refetch.
watch(
  () => props.refreshKey,
  () => fetchFaces(),
)

const assign = async (face: FaceRef, contactId: number | null) => {
  try {
    busyFaceId.value = face.id
    await faceService.assignFace(face.id, { contact_id: contactId })
    // Refetch, don't patch in place: under a filter (unassigned-only, by
    // contact) the row may belong on a different page now.
    await fetchFaces()
    emit('refresh')
  } catch (err: unknown) {
    alert('Failed to update label: ' + errorMessage(err))
    // Snap the select back to the stored label (unchanged on failure).
    await fetchFaces()
  } finally {
    busyFaceId.value = null
  }
}

const confirmFace = async (face: FaceRef) => {
  try {
    busyFaceId.value = face.id
    await faceService.confirmFace(face.id)
    await fetchFaces()
    emit('refresh')
  } catch (err: unknown) {
    alert('Failed to confirm face: ' + errorMessage(err))
  } finally {
    busyFaceId.value = null
  }
}

const withdrawFace = async (face: FaceRef) => {
  if (!confirm('Withdraw this face as a confirmed exemplar? Its label is kept.')) return

  try {
    busyFaceId.value = face.id
    await faceService.withdrawFace(face.id)
    await fetchFaces()
    emit('refresh')
  } catch (err: unknown) {
    alert('Failed to withdraw face: ' + errorMessage(err))
  } finally {
    busyFaceId.value = null
  }
}

const goPage = (delta: number) => {
  const next = page.value + delta
  if (next < 1 || next > totalPages.value) return
  page.value = next
  fetchFaces()
}

onMounted(fetchFaces)
</script>

<template>
  <div class="panel">
    <div class="filters glass-panel">
      <label class="filter">
        <span>Review queue (unassigned only)</span>
        <input v-model="filters.unassigned" type="checkbox" />
      </label>
      <label class="filter">
        <span>Contact</span>
        <select v-model.number="filters.contactId">
          <option value="">All</option>
          <option v-for="contact in contacts" :key="contact.id" :value="contact.id">
            {{ contact.name }}
          </option>
        </select>
      </label>
      <label class="filter">
        <span>Confirmed</span>
        <select v-model="filters.confirmed">
          <option value="all">All</option>
          <option value="yes">Exemplars only</option>
          <option value="no">Not confirmed</option>
        </select>
      </label>
      <button class="btn-secondary" title="Reload the current page" @click="fetchFaces">
        Refresh
      </button>
    </div>

    <div v-if="loading && faces.length === 0" class="loading-state">
      <div class="spinner"></div>
      <p>Loading faces…</p>
    </div>

    <div v-else-if="error" class="error-state glass-panel">
      <p>{{ error }}</p>
      <button class="btn-secondary" @click="fetchFaces">Retry</button>
    </div>

    <div v-else class="faces-table glass-panel">
      <table>
        <thead>
          <tr>
            <th>Face</th>
            <th>File hash</th>
            <th>BBox</th>
            <th>Model</th>
            <th>Label</th>
            <th>State</th>
            <th v-if="isAdmin">Actions</th>
          </tr>
        </thead>
        <tbody>
          <FaceRefRow
            v-for="face in faces"
            :key="face.id"
            :face="face"
            :contacts="contacts"
            :is-admin="isAdmin"
            :busy="busyFaceId === face.id"
            @assign="assign(face, $event)"
            @confirm="confirmFace(face)"
            @withdraw="withdrawFace(face)"
          />
        </tbody>
      </table>

      <div v-if="faces.length === 0" class="empty-state">
        <ScanFace :size="48" style="opacity: 0.3" />
        <p>No faces match the current filters.</p>
      </div>

      <div v-if="totalPages > 1" class="pagination">
        <button class="btn-secondary" :disabled="page <= 1" @click="goPage(-1)">
          <ChevronLeft :size="16" />
          <span>Prev</span>
        </button>
        <span class="mono">page {{ page }} of {{ totalPages }} · {{ total }} faces</span>
        <button class="btn-secondary" :disabled="page >= totalPages" @click="goPage(1)">
          <span>Next</span>
          <ChevronRight :size="16" />
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.panel {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.filters {
  display: flex;
  align-items: center;
  gap: 20px;
  padding: 14px 16px;
  flex-wrap: wrap;
}

.filter {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 0.875rem;
  color: var(--text-secondary);
}

.filter select {
  padding: 8px 10px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  color: var(--text-primary);
}

.filter input[type='checkbox'] {
  cursor: pointer;
}

.faces-table {
  padding: 0;
  overflow: hidden;
}

table {
  width: 100%;
  border-collapse: collapse;
}

thead {
  background: rgba(255, 255, 255, 0.02);
  border-bottom: 1px solid var(--border-color);
}

th {
  padding: 14px 16px;
  text-align: left;
  font-weight: 600;
  color: var(--text-secondary);
  font-size: 0.875rem;
  white-space: nowrap;
}

:deep(tbody td) {
  padding: 12px 16px;
  border-bottom: 1px solid var(--border-color);
  white-space: nowrap;
}

:deep(tbody tr:hover) {
  background: rgba(255, 255, 255, 0.02);
}

:deep(tbody tr:last-child td) {
  border-bottom: none;
}

.mono {
  font-family: monospace;
  font-size: 0.8125rem;
  color: var(--text-secondary);
}

.loading-state,
.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 60px;
  text-align: center;
  gap: 16px;
  color: var(--text-secondary);
}

.error-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 40px;
  gap: 16px;
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

.pagination {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 16px;
  padding: 14px;
  border-top: 1px solid var(--border-color);
}

.btn-secondary {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 14px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  color: var(--text-primary);
}

.btn-secondary:hover:not(:disabled) {
  border-color: var(--accent-color);
}

.btn-secondary:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
</style>
