<script setup lang="ts">
import { computed } from 'vue'
import { BadgeCheck, Undo2 } from 'lucide-vue-next'
import type { Contact, FaceRef } from '../api'

const props = defineProps<{
  face: FaceRef
  contacts: Contact[]
  isAdmin: boolean
  busy: boolean
}>()

const emit = defineEmits<{
  (e: 'assign', contactId: number | null): void
  (e: 'confirm'): void
  (e: 'withdraw'): void
}>()

const contactName = computed(() => {
  const id = props.face.contact_id
  if (id == null) return null
  return props.contacts.find((c) => c.id === id)?.name ?? `#${id}`
})

// The select's value mirrors the stored label; “Unassigned” clears it.
const onAssign = (event: Event) => {
  const raw = (event.target as HTMLSelectElement).value
  emit('assign', raw === '' ? null : Number(raw))
}
</script>

<template>
  <tr>
    <td><span class="mono">#{{ face.id }} · idx {{ face.face_index }}</span></td>
    <td><span class="hash" :title="face.hash">{{ face.hash.slice(0, 12) }}…</span></td>
    <td>
      <span class="hash">{{ face.bbox_x }},{{ face.bbox_y }} {{ face.bbox_w }}×{{ face.bbox_h }}</span>
    </td>
    <td>
      <span class="hash" :title="`${face.model_id}@${face.model_version}, dim ${face.dim}`">
        {{ face.model_id }}@{{ face.model_version }}
      </span>
    </td>
    <td>
      <select
        v-if="isAdmin"
        class="assign-select"
        :value="face.contact_id == null ? '' : face.contact_id"
        :disabled="busy"
        title="Label — “Unassigned” clears it (a confirmed exemplar is withdrawn too)"
        @change="onAssign"
      >
        <option value="">Unassigned</option>
        <option v-for="contact in contacts" :key="contact.id" :value="contact.id">
          {{ contact.name }}
        </option>
      </select>
      <template v-else>{{ contactName ?? 'Unassigned' }}</template>
    </td>
    <td>
      <span
        v-if="face.confirmed"
        class="state-badge confirmed"
        title="Confirmed exemplar — part of the matching pool"
      >exemplar</span>
      <span
        v-else-if="face.pinned"
        class="state-badge pinned"
        title="Human label — auto-matching never overwrites it"
      >pinned</span>
      <span v-else class="state-badge suggested" title="Machine suggestion — a re-match may change it">suggested</span>
    </td>
    <td v-if="isAdmin">
      <div class="actions">
        <button
          class="btn-icon"
          :disabled="busy || face.contact_id == null"
          :title="face.contact_id == null ? 'Assign a contact first' : 'Confirm as exemplar'"
          @click="emit('confirm')"
        >
          <BadgeCheck :size="16" />
        </button>
        <button
          class="btn-icon"
          :disabled="busy || !face.confirmed"
          title="Withdraw exemplar status (label is kept)"
          @click="emit('withdraw')"
        >
          <Undo2 :size="16" />
        </button>
      </div>
    </td>
  </tr>
</template>

<style scoped>
.hash,
.mono {
  font-family: monospace;
  font-size: 0.8125rem;
  color: var(--text-secondary);
}

.assign-select {
  max-width: 160px;
  padding: 6px 8px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  border-radius: 6px;
  color: var(--text-primary);
}

.state-badge {
  display: inline-block;
  padding: 3px 10px;
  border-radius: 20px;
  font-size: 0.75rem;
  font-weight: 500;
}

.state-badge.confirmed {
  background: rgba(16, 185, 129, 0.1);
  color: var(--success-color);
  border: 1px solid rgba(16, 185, 129, 0.2);
}

.state-badge.pinned {
  background: rgba(139, 92, 246, 0.1);
  color: #a78bfa;
  border: 1px solid rgba(139, 92, 246, 0.2);
}

.state-badge.suggested {
  background: rgba(107, 114, 128, 0.1);
  color: #9ca3af;
  border: 1px solid rgba(107, 114, 128, 0.2);
}

.actions {
  display: flex;
  gap: 8px;
}

.btn-icon {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 8px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  border-radius: 6px;
  color: var(--text-primary);
}

.btn-icon:hover:not(:disabled) {
  background: rgba(255, 255, 255, 0.08);
  border-color: var(--accent-color);
}

.btn-icon:disabled {
  opacity: 0.3;
  cursor: not-allowed;
}
</style>
