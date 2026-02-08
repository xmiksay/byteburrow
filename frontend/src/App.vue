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
  UserCircle,
  Folder,
  Tag as TagIcon,
  ChevronLeft,
  ChevronRight,
  Share2
} from 'lucide-vue-next'
import Login from './components/Login.vue'
import UserManagement from './components/UserManagement.vue'
import StorageManagement from './components/StorageManagement.vue'
import GroupManagement from './components/GroupManagement.vue'
import TagManagement from './components/TagManagement.vue'
import FileExplorer from './components/FileExplorer.vue'
import ShareManagement from './components/ShareManagement.vue'
import { useAuth } from './composables/useAuth'
import { api } from './utils/api'

const { isAuthenticated, user, logout, fetchUserInfo } = useAuth()

type View = 'files' | 'storages' | 'users' | 'groups' | 'tags' | 'monitoring' | 'shares'
const currentView = ref<View>('files')
const isSidebarCollapsed = ref(localStorage.getItem('sidebar_collapsed') === 'true')

const toggleSidebar = () => {
  isSidebarCollapsed.value = !isSidebarCollapsed.value
  localStorage.setItem('sidebar_collapsed', isSidebarCollapsed.value.toString())
}

const storages = ref<any[]>([])
const loading = ref(true)
const error = ref<string | null>(null)
const healthInfo = ref<any>(null)

const fetchHealth = async () => {
  try {
    const [health, version] = await Promise.all([
      api.get<any>('/api/health'),
      api.get<any>('/api/version')
    ])
    healthInfo.value = { ...health, ...version }
  } catch (err) {
    console.error('Failed to fetch health info:', err)
  }
}

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
    if (currentView.value === 'monitoring') fetchHealth()
  }
})

watch(currentView, (view) => {
  if (view === 'monitoring' && isAuthenticated.value) {
    fetchHealth()
  }
})

onMounted(async () => {
  // Try to restore session if token exists
  if (isAuthenticated.value || localStorage.getItem('auth_token')) {
    const success = await fetchUserInfo()
    if (success) {
      await fetchData()
      if (currentView.value === 'monitoring') await fetchHealth()
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
  <div v-else class="app-container" :class="{ 'sidebar-collapsed': isSidebarCollapsed }">
    <aside class="sidebar glass-panel">
      <div class="logo-section">
        <Cloud class="logo-icon" :size="28" />
        <h1 v-if="!isSidebarCollapsed">Cloud</h1>
      </div>

      <nav class="main-nav">
        <a
          href="#"
          class="nav-item"
          :class="{ active: currentView === 'files' }"
          @click.prevent="currentView = 'files'"
          title="Files"
        >
          <Folder :size="20" />
          <span v-if="!isSidebarCollapsed">Files</span>
        </a>
        <a
          href="#"
          class="nav-item"
          :class="{ active: currentView === 'storages' }"
          @click.prevent="currentView = 'storages'"
          title="Storages"
        >
          <Database :size="20" />
          <span v-if="!isSidebarCollapsed">Storages</span>
        </a>
        <a
          v-if="user?.admin"
          href="#"
          class="nav-item"
          :class="{ active: currentView === 'users' }"
          @click.prevent="currentView = 'users'"
          title="Users"
        >
          <Users :size="20" />
          <span v-if="!isSidebarCollapsed">Users</span>
        </a>
        <a
          v-if="user?.admin"
          href="#"
          class="nav-item"
          :class="{ active: currentView === 'groups' }"
          @click.prevent="currentView = 'groups'"
          title="Groups"
        >
          <Users :size="20" />
          <span v-if="!isSidebarCollapsed">Groups</span>
        </a>
        <a
          href="#"
          class="nav-item"
          :class="{ active: currentView === 'tags' }"
          @click.prevent="currentView = 'tags'"
          title="Tags"
        >
          <TagIcon :size="20" />
          <span v-if="!isSidebarCollapsed">Tags</span>
        </a>
        <a
          href="#"
          class="nav-item"
          :class="{ active: currentView === 'shares' }"
          @click.prevent="currentView = 'shares'"
          title="Shares"
        >
          <Share2 :size="20" />
          <span v-if="!isSidebarCollapsed">Shares</span>
        </a>
        <a
          href="#"
          class="nav-item"
          :class="{ active: currentView === 'monitoring' }"
          @click.prevent="currentView = 'monitoring'"
          title="Monitoring"
        >
          <Activity :size="20" />
          <span v-if="!isSidebarCollapsed">Monitoring</span>
        </a>
      </nav>

      <div class="spacer"></div>

      <button class="nav-item collapse-btn" @click="toggleSidebar">
        <ChevronLeft v-if="!isSidebarCollapsed" :size="20" />
        <ChevronRight v-else :size="20" />
        <span v-if="!isSidebarCollapsed">Collapse Sidebar</span>
      </button>

      <div class="nav-item settings">
        <Settings :size="20" />
        <span v-if="!isSidebarCollapsed">Settings</span>
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
        <!-- Files View -->
        <FileExplorer v-if="currentView === 'files'" />

        <!-- Storages View -->
        <StorageManagement v-if="currentView === 'storages' && user?.admin" />

        <!-- Non-admin storage view (read-only) -->
        <template v-else-if="currentView === 'storages' && !user?.admin">
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
                <span class="id-tag">#{{ storage.id }}</span>
              </div>
              <p class="description">{{ storage.path }}</p>
              <div class="card-footer">
                <span class="status-badge">Active</span>
              </div>
            </div>

            <div v-if="storages.length === 0" class="empty-state glass-panel fade-in">
              <Database :size="48" style="opacity: 0.3" />
              <p>No storage locations configured.</p>
              <span class="tip">Contact an administrator to add storage locations.</span>
            </div>
          </div>
        </template>

        <!-- Users View -->
        <UserManagement v-else-if="currentView === 'users' && user?.admin" />

        <!-- Groups View -->
        <GroupManagement v-else-if="currentView === 'groups' && user?.admin" />

        <!-- Tags View -->
        <TagManagement v-else-if="currentView === 'tags'" />

        <!-- Shares View -->
        <ShareManagement v-else-if="currentView === 'shares'" />

        <!-- Monitoring View -->
        <template v-else-if="currentView === 'monitoring'">
          <div class="area-header">
            <h2>System Monitoring</h2>
            <p>Real-time status of system components.</p>
          </div>
          
          <div v-if="healthInfo" class="grid-container">
            <div class="data-card glass-panel fade-in">
              <div class="card-header">
                <h3>Backend Service</h3>
                <span :class="['status-badge', healthInfo.status === 'ok' ? 'success' : 'danger']">
                  {{ healthInfo.status.toUpperCase() }}
                </span>
              </div>
              <p class="description">Core API service status and version information.</p>
              <div class="card-footer">
                <span class="info">Version: v{{ healthInfo.version || '0.0.0' }}</span>
                <span class="info" style="margin-left: 10px; opacity: 0.7">({{ healthInfo.commit?.substring(0, 7) || 'unknown' }})</span>
              </div>
            </div>

            <div class="data-card glass-panel fade-in">
              <div class="card-header">
                <h3>Database</h3>
                <span :class="['status-badge', healthInfo.database === 'ok' ? 'success' : 'danger']">
                  {{ healthInfo.database.toUpperCase() }}
                </span>
              </div>
              <p class="description">Connectivity to the primary data storage.</p>
              <div class="card-footer">
                <span class="info">Status: {{ healthInfo.database === 'ok' ? 'Connected' : 'Disconnected' }}</span>
              </div>
            </div>
          </div>
          
          <div v-else class="loading-state">
            <div class="spinner"></div>
            <p>Fetching system status...</p>
          </div>
        </template>
      </section>
    </main>
  </div>
</template>

<style scoped>
.app-container {
  display: grid;
  grid-template-columns: 240px 1fr;
  height: 100vh;
  gap: 16px;
  padding: 16px;
  transition: grid-template-columns 0.3s cubic-bezier(0.4, 0, 0.2, 1);
}

.app-container.sidebar-collapsed {
  grid-template-columns: 80px 1fr;
}

.sidebar {
  display: flex;
  flex-direction: column;
  padding: 20px 12px;
  overflow: hidden;
}

.logo-section {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 32px;
  padding: 0 12px;
}

.sidebar-collapsed .logo-section {
  justify-content: center;
  padding: 0;
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
  padding: 10px 12px;
  border-radius: 10px;
  color: var(--text-secondary);
  text-decoration: none;
  transition: all 0.2s;
  overflow: hidden;
  white-space: nowrap;
}

.sidebar-collapsed .nav-item {
  justify-content: center;
  padding: 12px;
  gap: 0;
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

.collapse-btn {
  border: none;
  background: none;
  cursor: pointer;
  width: 100%;
  margin-bottom: 8px;
  color: var(--text-secondary);
}

.collapse-btn:hover {
  background: rgba(255, 255, 255, 0.05);
  color: var(--text-primary);
}

.main-content {
  display: flex;
  flex-direction: column;
  gap: 24px;
  overflow-y: auto;
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.main-content::-webkit-scrollbar {
  display: none;
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
  gap: 10px;
  padding: 6px 12px;
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
  padding: 8px 14px;
  max-width: 320px;
  flex: 1;
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

.card-footer .info {
  font-size: 0.75rem;
  color: var(--text-secondary);
}

.status-badge.success {
  background: rgba(16, 185, 129, 0.1);
  color: var(--success-color);
  border-color: rgba(16, 185, 129, 0.2);
}

.status-badge.danger {
  background: rgba(239, 68, 68, 0.1);
  color: var(--danger-color);
  border-color: rgba(239, 68, 68, 0.2);
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
