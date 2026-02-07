<script setup lang="ts">
import { ref, onMounted, watch } from 'vue'
import {
  Cloud,
  Database,
  Settings,
  Users,
  Activity,
  Search,
  LogOut,
  UserCircle
} from 'lucide-vue-next'
import Login from './components/Login.vue'
import UserManagement from './components/UserManagement.vue'
import { useAuth } from './composables/useAuth'
import { api } from './utils/api'

const { isAuthenticated, user, logout, fetchUserInfo } = useAuth()

type View = 'storages' | 'users' | 'monitoring'
const currentView = ref<View>('storages')

const storages = ref<any[]>([])
const loading = ref(true)
const error = ref<string | null>(null)

const fetchData = async () => {
  try {
    loading.value = true
    error.value = null
    const response = await api.get<{ user: any; storages: any[] }>('/api/storage')
    storages.value = response.storages
  } catch (err: any) {
    error.value = err.message
    // If we get an auth error, the token might be invalid
    if (err.message.includes('401') || err.message.includes('Unauthorized')) {
      logout()
    }
  } finally {
    loading.value = false
  }
}

const handleLogout = () => {
  logout()
  storages.value = []
}

// Watch for authentication changes
watch(isAuthenticated, (authenticated) => {
  if (authenticated) {
    fetchData()
  }
})

onMounted(async () => {
  // Try to restore session if token exists
  if (isAuthenticated.value || localStorage.getItem('auth_token')) {
    const success = await fetchUserInfo()
    if (success) {
      await fetchData()
    } else {
      loading.value = false
    }
  } else {
    loading.value = false
  }
})
</script>

<template>
  <!-- Show login if not authenticated -->
  <Login v-if="!isAuthenticated" />

  <!-- Show main app if authenticated -->
  <div v-else class="app-container">
    <aside class="sidebar glass-panel">
      <div class="logo-section">
        <Cloud class="logo-icon" :size="32" />
        <h1>Cloud</h1>
      </div>

      <nav class="main-nav">
        <a
          href="#"
          class="nav-item"
          :class="{ active: currentView === 'storages' }"
          @click.prevent="currentView = 'storages'"
        >
          <Database :size="20" />
          <span>Storages</span>
        </a>
        <a
          v-if="user?.admin"
          href="#"
          class="nav-item"
          :class="{ active: currentView === 'users' }"
          @click.prevent="currentView = 'users'"
        >
          <Users :size="20" />
          <span>Users</span>
        </a>
        <a
          href="#"
          class="nav-item"
          :class="{ active: currentView === 'monitoring' }"
          @click.prevent="currentView = 'monitoring'"
        >
          <Activity :size="20" />
          <span>Monitoring</span>
        </a>
      </nav>

      <div class="spacer"></div>

      <div class="nav-item settings">
        <Settings :size="20" />
        <span>Settings</span>
      </div>
    </aside>

    <main class="main-content">
      <header class="top-header">
        <div class="search-bar glass-panel">
          <Search :size="18" />
          <input type="text" placeholder="Search..." />
        </div>

        <div class="header-actions">
          <div class="user-info glass-panel">
            <UserCircle :size="20" />
            <div class="user-details">
              <span class="user-name">{{ user?.name || user?.username }}</span>
              <span class="user-role">{{ user?.admin ? 'Admin' : 'User' }}</span>
            </div>
          </div>
          <button class="btn-secondary" @click="handleLogout" title="Logout">
            <LogOut :size="18" />
          </button>
        </div>
      </header>

      <section class="content-area">
        <!-- Storages View -->
        <template v-if="currentView === 'storages'">
          <div class="area-header">
            <h2>Storage Locations</h2>
            <p>Displaying all configured storage locations.</p>
          </div>

          <div v-if="loading" class="loading-state">
            <div class="spinner"></div>
            <p>Loading storage information...</p>
          </div>

          <div v-else-if="error" class="error-state glass-panel">
            <p>Error: {{ error }}</p>
            <button @click="fetchData" class="btn-primary">Retry</button>
          </div>

          <div v-else class="grid-container">
            <div v-for="storage in storages" :key="storage.id" class="data-card glass-panel fade-in">
              <div class="card-header">
                <h3>{{ storage.name }}</h3>
                <span class="id-tag">#{{ storage.id.substring(0, 8) }}</span>
              </div>
              <p class="description">{{ storage.path }}</p>
              <div class="card-footer">
                <span class="status-badge">Active</span>
              </div>
            </div>

            <div v-if="storages.length === 0" class="empty-state glass-panel fade-in">
              <Database :size="48" style="opacity: 0.3" />
              <p>No storage locations configured.</p>
              <span class="tip">Add storage locations to see them here.</span>
            </div>
          </div>
        </template>

        <!-- Users View -->
        <UserManagement v-else-if="currentView === 'users' && user?.admin" />

        <!-- Monitoring View -->
        <template v-else-if="currentView === 'monitoring'">
          <div class="area-header">
            <h2>Monitoring</h2>
            <p>System monitoring and analytics.</p>
          </div>
          <div class="empty-state glass-panel">
            <Activity :size="48" style="opacity: 0.3" />
            <p>Monitoring features coming soon.</p>
          </div>
        </template>
      </section>
    </main>
  </div>
</template>

<style scoped>
.app-container {
  display: grid;
  grid-template-columns: 260px 1fr;
  height: 100vh;
  gap: 20px;
  padding: 20px;
}

.sidebar {
  display: flex;
  flex-direction: column;
  padding: 24px;
}

.logo-section {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 40px;
}

.logo-icon {
  color: var(--accent-color);
}

.logo-section h1 {
  font-size: 1.5rem;
  font-weight: 700;
}

.main-nav {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.nav-item {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 16px;
  border-radius: 12px;
  color: var(--text-secondary);
  text-decoration: none;
  transition: all 0.2s;
}

.nav-item:hover, .nav-item.active {
  background: rgba(255, 255, 255, 0.05);
  color: var(--text-primary);
}

.nav-item.active {
  background: rgba(59, 130, 246, 0.1);
  color: var(--accent-color);
  box-shadow: inset 0 0 0 1px rgba(59, 130, 246, 0.2);
}

.spacer {
  flex: 1;
}

.settings {
  margin-top: auto;
}

.main-content {
  display: flex;
  flex-direction: column;
  gap: 30px;
  overflow-y: auto;
}

.top-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.header-actions {
  display: flex;
  align-items: center;
  gap: 12px;
}

.user-info {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 8px 16px;
}

.user-details {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.user-name {
  font-size: 0.875rem;
  font-weight: 500;
  color: var(--text-primary);
}

.user-role {
  font-size: 0.75rem;
  color: var(--text-secondary);
}

.btn-secondary {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 16px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  color: var(--text-primary);
  cursor: pointer;
  transition: all 0.2s;
  font-size: 0.875rem;
  font-weight: 500;
}

.btn-secondary:hover {
  background: rgba(255, 255, 255, 0.08);
  border-color: var(--accent-color);
}

.btn-secondary:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.search-bar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 16px;
  width: 400px;
}

.search-bar input {
  background: none;
  border: none;
  color: white;
  width: 100%;
  outline: none;
}

.content-area {
  display: flex;
  flex-direction: column;
  gap: 24px;
}

.area-header h2 {
  font-size: 1.75rem;
  margin-bottom: 4px;
}

.area-header p {
  color: var(--text-secondary);
}

.grid-container {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
  gap: 20px;
}

.data-card {
  padding: 24px;
  display: flex;
  flex-direction: column;
  gap: 16px;
  transition: transform 0.2s;
}

.data-card:hover {
  transform: translateY(-4px);
  border-color: var(--accent-color);
}

.card-header {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
}

.id-tag {
  font-family: monospace;
  font-size: 0.75rem;
  color: var(--text-secondary);
  background: rgba(255, 255, 255, 0.05);
  padding: 2px 6px;
  border-radius: 4px;
}

.description {
  color: var(--text-secondary);
  font-size: 0.875rem;
}

.card-footer {
  margin-top: auto;
  padding-top: 16px;
  border-top: 1px solid var(--border-color);
}

.status-badge {
  font-size: 0.75rem;
  padding: 4px 10px;
  background: rgba(16, 185, 129, 0.1);
  color: var(--success-color);
  border-radius: 20px;
  border: 1px solid rgba(16, 185, 129, 0.2);
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

.empty-state .tip {
  font-size: 0.875rem;
  color: var(--text-secondary);
  max-width: 300px;
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

@media (max-width: 1024px) {
  .app-container {
    grid-template-columns: 1fr;
  }
  .sidebar {
    display: none;
  }
}
</style>
