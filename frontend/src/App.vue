<script setup lang="ts">
import { ref, onMounted, watch, computed } from 'vue'
import { useRouter, useRoute } from 'vue-router'
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
import { useAuth } from './composables/useAuth'
import { api } from './utils/api'

const router = useRouter()
const route = useRoute()
const { isAuthenticated, user, logout, fetchUserInfo } = useAuth()

const isSidebarCollapsed = ref(localStorage.getItem('sidebar_collapsed') === 'true')

const toggleSidebar = () => {
  isSidebarCollapsed.value = !isSidebarCollapsed.value
  localStorage.setItem('sidebar_collapsed', isSidebarCollapsed.value.toString())
}

const storages = ref<any[]>([])
const loading = ref(true)
const error = ref<string | null>(null)
const healthInfo = ref<any>(null)

const currentRouteName = computed(() => route.name as string)

const isNavActive = (name: string) => currentRouteName.value === name

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
  router.push('/files')
}

// Watch for route changes to fetch monitoring data
watch(currentRouteName, (name) => {
  if (name === 'monitoring' && isAuthenticated.value) {
    fetchHealth()
  }
})

// Watch for authentication changes
watch(isAuthenticated, (authenticated) => {
  if (authenticated) {
    fetchData()
    if (currentRouteName.value === 'monitoring') fetchHealth()
  }
})

onMounted(async () => {
  // Try to restore session if token exists
  if (isAuthenticated.value || localStorage.getItem('auth_token')) {
    const success = await fetchUserInfo()
    if (success) {
      await fetchData()
      if (currentRouteName.value === 'monitoring') await fetchHealth()
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
        <router-link
          to="/files"
          class="nav-item"
          :class="{ active: isNavActive('files') }"
          title="Files"
        >
          <Folder :size="20" />
          <span v-if="!isSidebarCollapsed">Files</span>
        </router-link>
        <router-link
          to="/shared"
          class="nav-item"
          :class="{ active: isNavActive('shared') }"
          title="Shared with Me"
        >
          <Share2 :size="20" />
          <span v-if="!isSidebarCollapsed">Shared with Me</span>
        </router-link>
        <router-link
          to="/storages"
          class="nav-item"
          :class="{ active: isNavActive('storages') }"
          title="Storages"
        >
          <Database :size="20" />
          <span v-if="!isSidebarCollapsed">Storages</span>
        </router-link>

        <router-link
          v-if="user?.admin"
          to="/users"
          class="nav-item"
          :class="{ active: isNavActive('users') }"
          title="Users"
        >
          <Users :size="20" />
          <span v-if="!isSidebarCollapsed">Users</span>
        </router-link>
        <router-link
          v-if="user?.admin"
          to="/groups"
          class="nav-item"
          :class="{ active: isNavActive('groups') }"
          title="Groups"
        >
          <Users :size="20" />
          <span v-if="!isSidebarCollapsed">Groups</span>
        </router-link>
        <router-link
          to="/tags"
          class="nav-item"
          :class="{ active: isNavActive('tags') }"
          title="Tags"
        >
          <TagIcon :size="20" />
          <span v-if="!isSidebarCollapsed">Tags</span>
        </router-link>
        <router-link
          to="/shares"
          class="nav-item"
          :class="{ active: isNavActive('shares') }"
          title="Shares"
        >
          <Share2 :size="20" />
          <span v-if="!isSidebarCollapsed">Shares</span>
        </router-link>
        <router-link
          to="/monitoring"
          class="nav-item"
          :class="{ active: isNavActive('monitoring') }"
          title="Monitoring"
        >
          <Activity :size="20" />
          <span v-if="!isSidebarCollapsed">Monitoring</span>
        </router-link>
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
        <router-view v-slot="{ Component }">
          <transition name="fade" mode="out-in">
            <component :is="Component" />
          </transition>
        </router-view>
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

h1 {
  font-size: 1.25rem;
  font-weight: 600;
  background: linear-gradient(135deg, var(--accent-color), #a78bfa);
  -webkit-background-clip: text;
  -webkit-text-fill-color: transparent;
  white-space: nowrap;
}

.main-nav {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.nav-item {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 14px;
  border-radius: 10px;
  color: var(--text-secondary);
  transition: all 0.2s;
  cursor: pointer;
  border: none;
  background: transparent;
  text-align: left;
  text-decoration: none;
  width: 100%;
}

.sidebar-collapsed .nav-item {
  justify-content: center;
  padding: 12px;
}

.nav-item:hover {
  background: rgba(255, 255, 255, 0.05);
  color: var(--text-primary);
}

.nav-item.active {
  background: rgba(59, 130, 246, 0.15);
  color: var(--accent-color);
}

.nav-item span {
  white-space: nowrap;
  overflow: hidden;
}

.spacer {
  flex: 1;
}

.collapse-btn {
  margin-top: auto;
}

.settings {
  margin-top: 8px;
  opacity: 0.6;
}

.main-content {
  display: flex;
  flex-direction: column;
  gap: 16px;
  overflow: hidden;
}

.top-header {
  display: flex;
  align-items: center;
  gap: 16px;
}

.search-bar {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 16px;
}

.search-bar input {
  flex: 1;
  background: none;
  border: none;
  color: var(--text-primary);
  outline: none;
}

.search-bar input::placeholder {
  color: var(--text-secondary);
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
  padding: 8px 16px;
}

.user-details {
  display: flex;
  flex-direction: column;
}

.user-name {
  font-weight: 500;
  font-size: 0.85rem;
}

.user-role {
  font-size: 0.7rem;
  color: var(--text-secondary);
}

.content-area {
  flex: 1;
  overflow-x: hidden;
  overflow-y: auto;
  padding-right: 4px;
  scrollbar-width: none;
}

.content-area::-webkit-scrollbar {
  display: none;
}

/* Route transition */
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.2s ease;
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
