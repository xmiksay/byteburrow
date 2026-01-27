<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { 
  Cloud, 
  Database, 
  Settings, 
  Users, 
  Activity, 
  Search,
  Plus
} from 'lucide-vue-next'

const items = ref<any[]>([])
const loading = ref(true)
const error = ref<string | null>(null)

const fetchItems = async () => {
  try {
    loading.value = true
    const response = await fetch('/api/dummy')
    if (!response.ok) throw new Error('Failed to fetch data')
    items.value = await response.json()
  } catch (err: any) {
    error.value = err.message
  } finally {
    loading.value = false
  }
}

onMounted(() => {
  fetchItems()
})
</script>

<template>
  <div class="app-container">
    <aside class="sidebar glass-panel">
      <div class="logo-section">
        <Cloud class="logo-icon" :size="32" />
        <h1>Cloud</h1>
      </div>
      
      <nav class="main-nav">
        <a href="#" class="nav-item active">
          <Database :size="20" />
          <span>Entities</span>
        </a>
        <a href="#" class="nav-item">
          <Users :size="20" />
          <span>Users</span>
        </a>
        <a href="#" class="nav-item">
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
          <input type="text" placeholder="Search entities..." />
        </div>
        <button class="btn-primary">
          <Plus :size="18" />
          New Entity
        </button>
      </header>
      
      <section class="content-area">
        <div class="area-header">
          <h2>Dummy Entities</h2>
          <p>Displaying all records from the database.</p>
        </div>
        
        <div v-if="loading" class="loading-state">
          <div class="spinner"></div>
          <p>Loading database information...</p>
        </div>
        
        <div v-else-if="error" class="error-state glass-panel">
          <p>Error: {{ error }}</p>
          <button @click="fetchItems">Retry</button>
        </div>
        
        <div v-else class="grid-container">
          <div v-for="item in items" :key="item.id" class="data-card glass-panel fade-in">
            <div class="card-header">
              <h3>{{ item.name }}</h3>
              <span class="id-tag">#{{ item.id.substring(0, 8) }}</span>
            </div>
            <p class="description">{{ item.description || 'No description available' }}</p>
            <div class="card-footer">
              <span class="status-badge">Active</span>
            </div>
          </div>
          
          <div v-if="items.length === 0" class="empty-state glass-panel fade-in">
            <Database :size="48" style="opacity: 0.3" />
            <p>No entities found in the database.</p>
            <span class="tip">Run migrations or add data via SQL to see records here.</span>
          </div>
        </div>
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
