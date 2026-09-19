<script setup lang="ts">
import { X, Save } from 'lucide-vue-next'

defineProps<{
  mode: 'create' | 'rename'
  name: string
  error: string | null
  submitting: boolean
}>()

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'submit'): void
  (e: 'update:name', value: string): void
}>()
</script>

<template>
  <div class="modal-overlay" @click.self="emit('close')">
    <div class="modal glass-panel">
      <div class="modal-header">
        <h3>{{ mode === 'create' ? 'New Contact' : 'Rename Contact' }}</h3>
        <button class="btn-icon" @click="emit('close')">
          <X :size="20" />
        </button>
      </div>

      <form class="modal-body" @submit.prevent="emit('submit')">
        <div class="form-group">
          <label for="contactName">Name</label>
          <input
            id="contactName"
            :value="name"
            type="text"
            placeholder="e.g. Ada Lovelace"
            autofocus
            @input="emit('update:name', ($event.target as HTMLInputElement).value)"
          />
          <span v-if="error" class="error-message">{{ error }}</span>
        </div>

        <div class="modal-footer">
          <button type="button" class="btn-secondary" :disabled="submitting" @click="emit('close')">
            Cancel
          </button>
          <button type="submit" class="btn-primary" :disabled="submitting || !name.trim()">
            <Save :size="16" />
            <span>{{ submitting ? 'Saving…' : 'Save' }}</span>
          </button>
        </div>
      </form>
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
  z-index: 1000;
  padding: 20px;
}

.modal {
  width: 100%;
  max-width: 420px;
}

.modal-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 20px 24px;
  border-bottom: 1px solid var(--border-color);
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
}

.form-group input:focus {
  border-color: var(--accent-color);
}

.error-message {
  font-size: 0.75rem;
  color: #ef4444;
}

.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: 12px;
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
}

.btn-secondary:hover:not(:disabled) {
  border-color: var(--accent-color);
}

.btn-secondary:disabled {
  opacity: 0.5;
  cursor: not-allowed;
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
  border-color: var(--accent-color);
}
</style>
