<script setup lang="ts">
import { ref } from 'vue'
import { useAuth } from '../composables/useAuth'
import { LogIn, User, Lock, AlertCircle } from 'lucide-vue-next'

const { login, isLoading, error, clearError } = useAuth()

const username = ref('')
const password = ref('')

const handleSubmit = async () => {
  clearError()
  await login(username.value, password.value)
}
</script>

<template>
  <div class="login-container">
    <div class="login-card glass-panel">
      <div class="login-header">
        <LogIn :size="48" class="login-icon" />
        <h1>Cloud Login</h1>
        <p>Sign in to access your cloud storage</p>
      </div>

      <form @submit.prevent="handleSubmit" class="login-form">
        <div class="form-group">
          <label for="username">
            <User :size="18" />
            <span>Username</span>
          </label>
          <input
            id="username"
            v-model="username"
            type="text"
            placeholder="Enter your username"
            required
            :disabled="isLoading"
            autocomplete="username"
          />
        </div>

        <div class="form-group">
          <label for="password">
            <Lock :size="18" />
            <span>Password</span>
          </label>
          <input
            id="password"
            v-model="password"
            type="password"
            placeholder="Enter your password"
            required
            :disabled="isLoading"
            autocomplete="current-password"
          />
        </div>

        <div v-if="error" class="error-message">
          <AlertCircle :size="16" />
          <span>{{ error }}</span>
        </div>

        <button type="submit" class="btn-primary btn-block" :disabled="isLoading">
          <LogIn :size="18" v-if="!isLoading" />
          <div v-else class="spinner-small"></div>
          <span>{{ isLoading ? 'Signing in...' : 'Sign In' }}</span>
        </button>
      </form>
    </div>
  </div>
</template>

<style scoped>
.login-container {
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 100vh;
  padding: 20px;
}

.login-card {
  width: 100%;
  max-width: 440px;
  padding: 48px;
}

.login-header {
  text-align: center;
  margin-bottom: 40px;
}

.login-icon {
  color: var(--accent-color);
  margin-bottom: 16px;
}

.login-header h1 {
  font-size: 2rem;
  margin-bottom: 8px;
}

.login-header p {
  color: var(--text-secondary);
  font-size: 0.875rem;
}

.login-form {
  display: flex;
  flex-direction: column;
  gap: 24px;
}

.form-group {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.form-group label {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 0.875rem;
  font-weight: 500;
  color: var(--text-secondary);
}

.form-group input {
  padding: 12px 16px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  color: white;
  font-size: 1rem;
  transition: all 0.2s;
}

.form-group input:focus {
  outline: none;
  border-color: var(--accent-color);
  background: rgba(255, 255, 255, 0.08);
}

.form-group input:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.error-message {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 12px 16px;
  background: rgba(239, 68, 68, 0.1);
  border: 1px solid rgba(239, 68, 68, 0.3);
  border-radius: 8px;
  color: #fca5a5;
  font-size: 0.875rem;
}

.btn-block {
  width: 100%;
  justify-content: center;
}

.spinner-small {
  width: 18px;
  height: 18px;
  border: 2px solid rgba(255, 255, 255, 0.3);
  border-top-color: white;
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}
</style>
