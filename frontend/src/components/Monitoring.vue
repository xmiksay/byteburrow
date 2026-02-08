<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { api } from '../utils/api'

const healthInfo = ref<any>(null)
const loading = ref(true)

const fetchHealth = async () => {
  try {
    loading.value = true
    const [health, version] = await Promise.all([
      api.get<any>('/api/health'),
      api.get<any>('/api/version')
    ])
    healthInfo.value = { ...health, ...version }
  } catch (err) {
    console.error('Failed to fetch health info:', err)
  } finally {
    loading.value = false
  }
}

onMounted(fetchHealth)
</script>

<template>
  <div class="monitoring">
    <div class="area-header">
      <h2>System Monitoring</h2>
      <p>Real-time status of system components.</p>
    </div>
    
    <div v-if="loading" class="loading-state">
      <div class="spinner"></div>
      <p>Fetching system status...</p>
    </div>
    
    <div v-else-if="healthInfo" class="grid-container fade-in">
      <div class="data-card glass-panel">
        <div class="card-header">
          <h3>Backend Service</h3>
          <span :class="['status-badge', healthInfo.status === 'ok' ? 'success' : 'danger']">
            {{ healthInfo.status?.toUpperCase() }}
          </span>
        </div>
        <p class="description">Core API service status and version information.</p>
        <div class="card-footer">
          <span class="info">Version: v{{ healthInfo.version || '0.0.0' }}</span>
          <span class="info" style="margin-left: 10px; opacity: 0.7">({{ healthInfo.commit?.substring(0, 7) || 'unknown' }})</span>
        </div>
      </div>

      <div class="data-card glass-panel">
        <div class="card-header">
          <h3>Database</h3>
          <span :class="['status-badge', healthInfo.database === 'ok' ? 'success' : 'danger']">
            {{ healthInfo.database?.toUpperCase() }}
          </span>
        </div>
        <p class="description">Connectivity to the primary data storage.</p>
        <div class="card-footer">
          <span class="info">Status: {{ healthInfo.database === 'ok' ? 'Connected' : 'Disconnected' }}</span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.monitoring {
  display: flex;
  flex-direction: column;
  gap: 24px;
}

.grid-container {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
  gap: 16px;
}

.data-card {
  padding: 20px;
}

.card-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 12px;
}

.card-header h3 {
  margin: 0;
  font-size: 1rem;
}

.description {
  color: var(--text-secondary);
  font-size: 0.85rem;
  margin-bottom: 16px;
}

.card-footer {
  display: flex;
  align-items: center;
}

.info {
  font-size: 0.8rem;
  color: var(--text-secondary);
}

.status-badge {
  padding: 4px 10px;
  border-radius: 20px;
  font-size: 0.7rem;
  font-weight: 600;
}

.status-badge.success {
  background: rgba(16, 185, 129, 0.1);
  color: var(--success-color);
}

.status-badge.danger {
  background: rgba(239, 68, 68, 0.1);
  color: var(--danger-color);
}

.loading-state {
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
