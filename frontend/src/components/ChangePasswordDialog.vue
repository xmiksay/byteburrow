<script setup lang="ts">
import { ref } from 'vue'
import { userService } from '../services/user'

const props = defineProps<{
  userId: number
  userName: string
}>()

const emit = defineEmits(['close', 'success'])

const password = ref('')
const confirmPassword = ref('')
const error = ref('')
const loading = ref(false)

const save = async () => {
    if (password.value !== confirmPassword.value) {
        error.value = "Passwords do not match"
        return
    }
    if (!password.value) {
        error.value = "Password cannot be empty"
        return
    }

    loading.value = true
    error.value = ''
    try {
        await userService.changePassword(props.userId, password.value)
        emit('success')
        emit('close')
    } catch (e: any) {
        error.value = e.message || 'Failed to change password'
    } finally {
        loading.value = false
    }
}
</script>

<template>
  <div class="modal-overlay" @click.self="$emit('close')">
    <div class="modal glass-panel">
        <div class="modal-header">
            <h3>Change Password for {{ userName }}</h3>
            <button class="btn-icon close-btn" @click="$emit('close')">×</button>
        </div>
        <div class="modal-body">
            <div v-if="error" class="error-message">{{ error }}</div>
            
            <div class="form-group">
                <label class="form-label">New Password</label>
                <input type="password" v-model="password" class="form-input" autofocus placeholder="Enter new password">
            </div>
            <div class="form-group">
                <label class="form-label">Confirm Password</label>
                <input type="password" v-model="confirmPassword" class="form-input" @keyup.enter="save" placeholder="Confirm new password">
            </div>
        </div>
        <div class="modal-footer">
            <button class="btn-secondary" @click="$emit('close')">Cancel</button>
            <button class="btn-primary" @click="save" :disabled="loading || !password">
                {{ loading ? 'Changing...' : 'Change Password' }}
            </button>
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
  z-index: 100;
}

.modal {
  width: 90%;
  max-width: 400px;
  display: flex;
  flex-direction: column;
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
  background: none;
  border: none;
  color: var(--text-secondary);
  font-size: 1.5rem;
  cursor: pointer;
  padding: 0;
  line-height: 1;
}

.close-btn:hover {
  color: var(--text-primary);
}

.modal-body {
  padding: 20px;
}

.form-group {
  margin-bottom: 16px;
}

.form-label {
  display: block;
  margin-bottom: 8px;
  font-weight: 500;
  font-size: 0.85rem;
  color: var(--text-secondary);
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
  transition: border-color 0.2s;
}

.form-input:focus {
  border-color: var(--accent-color);
}

.error-message {
  color: var(--danger-color);
  background: rgba(239, 68, 68, 0.1);
  padding: 10px;
  border-radius: 6px;
  margin-bottom: 16px;
  font-size: 0.85rem;
}

.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: 12px;
  padding: 16px 20px;
  border-top: 1px solid var(--border-color);
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
</style>
