<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { Search, ChevronDown, Check, User as UserIcon } from 'lucide-vue-next'
import { userService } from '../services/user'
import type { User } from '../types'

const props = defineProps<{
  modelValue: number
  error?: string
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', value: number): void
}>()

const users = ref<User[]>([])
const loading = ref(false)
const isOpen = ref(false)
const searchQuery = ref('')
const dropdownRef = ref<HTMLElement | null>(null)

const selectedUser = computed(() => 
  users.value.find(u => u.id === props.modelValue)
)

const filteredUsers = computed(() => {
  if (!searchQuery.value) return users.value
  const query = searchQuery.value.toLowerCase()
  return users.value.filter(u => 
    u.name.toLowerCase().includes(query) || 
    u.username.toLowerCase().includes(query)
  )
})

const fetchUsers = async () => {
  try {
    loading.value = true
    users.value = await userService.getUsers()
  } catch (err) {
    console.error('Failed to fetch users:', err)
  } finally {
    loading.value = false
  }
}

const selectUser = (user: User) => {
  emit('update:modelValue', user.id)
  isOpen.value = false
  searchQuery.value = ''
}

const toggleDropdown = () => {
  isOpen.value = !isOpen.value
}

const handleClickOutside = (event: MouseEvent) => {
  if (dropdownRef.value && !dropdownRef.value.contains(event.target as Node)) {
    isOpen.value = false
  }
}

onMounted(() => {
  fetchUsers()
  document.addEventListener('mousedown', handleClickOutside)
})

onUnmounted(() => {
  document.removeEventListener('mousedown', handleClickOutside)
})
</script>

<template>
  <div class="custom-select" ref="dropdownRef">
    <div 
      class="select-trigger" 
      :class="{ 'is-open': isOpen, 'has-error': error }"
      @click="toggleDropdown"
    >
      <div class="selected-value">
        <template v-if="selectedUser">
          <UserIcon :size="16" class="user-icon" />
          <span>{{ selectedUser.name }}</span>
          <span class="username-hint">({{ selectedUser.username }})</span>
        </template>
        <span v-else class="placeholder">Select user...</span>
      </div>
      <ChevronDown :size="18" class="chevron" />
    </div>

    <div v-if="isOpen" class="select-dropdown glass-panel fade-in">
      <div class="search-box">
        <Search :size="14" class="search-icon" />
        <input 
          v-model="searchQuery" 
          type="text" 
          placeholder="Search users..." 
          @click.stop
          autofocus
        />
      </div>
      
      <div class="options-list">
        <div v-if="loading" class="loading-state">
          <span>Loading...</span>
        </div>
        <template v-else-if="filteredUsers.length > 0">
          <div 
            v-for="user in filteredUsers" 
            :key="user.id"
            class="option"
            :class="{ 'is-selected': user.id === modelValue }"
            @click="selectUser(user)"
          >
            <div class="option-content">
              <span class="option-name">{{ user.name }}</span>
              <span class="option-username">{{ user.username }}</span>
            </div>
            <Check v-if="user.id === modelValue" :size="16" class="check-icon" />
          </div>
        </template>
        <div v-else class="empty-state">
          No users found
        </div>
      </div>
    </div>
    
    <span v-if="error" class="error-message">{{ error }}</span>
  </div>
</template>

<style scoped>
.custom-select {
  position: relative;
  width: 100%;
}

.select-trigger {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 12px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  cursor: pointer;
  transition: all 0.2s;
}

.select-trigger:hover {
  background: rgba(255, 255, 255, 0.08);
  border-color: var(--accent-color);
}

.select-trigger.is-open {
  border-color: var(--accent-color);
  background: rgba(255, 255, 255, 0.1);
}

.select-trigger.has-error {
  border-color: var(--danger-color);
}

.selected-value {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 0.875rem;
}

.user-icon {
  color: var(--accent-color);
  opacity: 0.7;
}

.username-hint {
  color: var(--text-secondary);
  font-size: 0.75rem;
  font-family: monospace;
}

.placeholder {
  color: var(--text-secondary);
}

.chevron {
  color: var(--text-secondary);
  transition: transform 0.2s;
}

.is-open .chevron {
  transform: rotate(180deg);
}

.select-dropdown {
  position: absolute;
  top: calc(100% + 8px);
  left: 0;
  right: 0;
  z-index: 100;
  display: flex;
  flex-direction: column;
  max-height: 300px;
  background: #121212;
  border-radius: 12px;
  overflow: hidden;
}

.search-box {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 12px;
  border-bottom: 1px solid var(--border-color);
}

.search-icon {
  color: var(--text-secondary);
}

.search-box input {
  flex: 1;
  background: transparent;
  border: none;
  color: var(--text-primary);
  font-size: 0.875rem;
  outline: none;
}

.options-list {
  overflow-y: auto;
  padding: 6px;
}

.option {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 12px;
  border-radius: 6px;
  cursor: pointer;
  transition: background 0.2s;
}

.option:hover {
  background: rgba(255, 255, 255, 0.05);
}

.option.is-selected {
  background: rgba(59, 130, 246, 0.1);
}

.option-content {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.option-name {
  font-size: 0.875rem;
  font-weight: 500;
}

.option-username {
  font-size: 0.75rem;
  color: var(--text-secondary);
  font-family: monospace;
}

.check-icon {
  color: var(--accent-color);
}

.loading-state, .empty-state {
  padding: 20px;
  text-align: center;
  color: var(--text-secondary);
  font-size: 0.875rem;
}

.error-message {
  font-size: 0.75rem;
  color: var(--danger-color);
  margin-top: 4px;
}

.fade-in {
  animation: fadeIn 0.2s ease-out;
}

@keyframes fadeIn {
  from { opacity: 0; transform: translateY(-10px); }
  to { opacity: 1; transform: translateY(0); }
}
</style>
