<script setup lang="ts">
import { ref } from 'vue'
import { Users, Plus, Edit2, Trash2 } from 'lucide-vue-next'
import { faceService } from '../services/face'
import { errorMessage } from '../utils/errors'
import FaceContactModal from './FaceContactModal.vue'
import type { Contact } from '../api'

defineProps<{
  contacts: Contact[]
  isAdmin: boolean
}>()

const emit = defineEmits<{
  (e: 'refresh'): void
}>()

// Create/rename modal state
const showModal = ref(false)
const modalMode = ref<'create' | 'rename'>('create')
const contactName = ref('')
const editingContactId = ref<number | null>(null)
const modalError = ref<string | null>(null)
const submitting = ref(false)

const openCreateModal = () => {
  modalMode.value = 'create'
  editingContactId.value = null
  contactName.value = ''
  modalError.value = null
  showModal.value = true
}

const openRenameModal = (contact: Contact) => {
  modalMode.value = 'rename'
  editingContactId.value = contact.id
  contactName.value = contact.name
  modalError.value = null
  showModal.value = true
}

const handleSubmit = async () => {
  const name = contactName.value.trim()
  if (!name) return

  try {
    submitting.value = true
    modalError.value = null
    if (modalMode.value === 'create') {
      await faceService.createContact({ name })
    } else if (editingContactId.value !== null) {
      await faceService.renameContact(editingContactId.value, { name })
    }
    showModal.value = false
    emit('refresh')
  } catch (err: unknown) {
    modalError.value = errorMessage(err)
  } finally {
    submitting.value = false
  }
}

const deleteContact = async (contact: Contact) => {
  const warning =
    contact.total_faces > 0
      ? `\n\nIts ${contact.total_faces} labeled face(s) lose their label and a re-match is queued.`
      : '\n\nA re-match is queued.'
  if (!confirm(`Delete contact "${contact.name}"?${warning}`)) return

  try {
    await faceService.deleteContact(contact.id)
    emit('refresh')
  } catch (err: unknown) {
    alert('Failed to delete contact: ' + errorMessage(err))
  }
}
</script>

<template>
  <div class="panel">
    <div class="panel-header">
      <p class="hint">
        A contact is a named person. Confirm a few good faces as exemplars per contact and the
        matcher labels the rest automatically.
      </p>
      <button v-if="isAdmin" class="btn-primary" @click="openCreateModal">
        <Plus :size="18" />
        <span>New Contact</span>
      </button>
    </div>

    <div class="contacts-table glass-panel">
      <table>
        <thead>
          <tr>
            <th>Name</th>
            <th>Confirmed exemplars</th>
            <th>Labeled faces</th>
            <th v-if="isAdmin">Actions</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="contact in contacts" :key="contact.id">
            <td>
              <div class="name-cell">
                <Users :size="16" />
                <span>{{ contact.name }}</span>
                <span class="id-tag">#{{ contact.id }}</span>
              </div>
            </td>
            <td>
              <span :class="['count-badge', { zero: contact.confirmed_faces === 0 }]">
                {{ contact.confirmed_faces }}
              </span>
            </td>
            <td>{{ contact.total_faces }}</td>
            <td v-if="isAdmin">
              <div class="actions">
                <button class="btn-icon" title="Rename" @click="openRenameModal(contact)">
                  <Edit2 :size="16" />
                </button>
                <button class="btn-icon danger" title="Delete" @click="deleteContact(contact)">
                  <Trash2 :size="16" />
                </button>
              </div>
            </td>
          </tr>
        </tbody>
      </table>

      <div v-if="contacts.length === 0" class="empty-state">
        <Users :size="48" style="opacity: 0.3" />
        <p>
          {{ isAdmin ? 'No contacts yet — create one to start labeling faces.' : 'No contacts yet.' }}
        </p>
        <button v-if="isAdmin" class="btn-primary" @click="openCreateModal">
          Create your first contact
        </button>
      </div>
    </div>

    <FaceContactModal
      v-if="showModal"
      v-model:name="contactName"
      :mode="modalMode"
      :error="modalError"
      :submitting="submitting"
      @close="showModal = false"
      @submit="handleSubmit"
    />
  </div>
</template>

<style scoped>
.panel {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.panel-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 16px;
}

.hint {
  color: var(--text-secondary);
  font-size: 0.875rem;
}

.btn-primary {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 20px;
  background: var(--accent-color);
  border: none;
  border-radius: 8px;
  color: white;
  font-weight: 500;
}

.btn-primary:hover:not(:disabled) {
  background: rgba(59, 130, 246, 0.8);
  transform: translateY(-2px);
}

.btn-primary:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.contacts-table {
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
}

td {
  padding: 14px 16px;
  border-bottom: 1px solid var(--border-color);
}

tbody tr:hover {
  background: rgba(255, 255, 255, 0.02);
}

tbody tr:last-child td {
  border-bottom: none;
}

.name-cell {
  display: flex;
  align-items: center;
  gap: 8px;
  font-weight: 500;
  color: var(--accent-color);
}

.name-cell span:first-of-type {
  color: var(--text-primary);
}

.id-tag {
  font-size: 0.75rem;
  color: var(--text-secondary);
}

.count-badge {
  display: inline-block;
  min-width: 32px;
  text-align: center;
  padding: 2px 10px;
  border-radius: 20px;
  font-size: 0.8125rem;
  background: rgba(16, 185, 129, 0.1);
  color: var(--success-color);
  border: 1px solid rgba(16, 185, 129, 0.2);
}

.count-badge.zero {
  background: rgba(107, 114, 128, 0.1);
  color: #9ca3af;
  border-color: rgba(107, 114, 128, 0.2);
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

.btn-icon:hover {
  background: rgba(255, 255, 255, 0.08);
  border-color: var(--accent-color);
}

.btn-icon.danger:hover {
  background: rgba(239, 68, 68, 0.1);
  border-color: #ef4444;
  color: #ef4444;
}

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
</style>
