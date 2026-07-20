import { ref, computed } from 'vue'

export interface User {
  id: number
  username: string
  name: string
  admin: boolean
}

export interface LoginResponse {
  expires_in_days: number
}

const user = ref<User | null>(null)
const isLoading = ref(false)
const error = ref<string | null>(null)

export function useAuth() {
  const isAuthenticated = computed(() => !!user.value)

  const login = async (username: string, password: string): Promise<boolean> => {
    try {
      isLoading.value = true
      error.value = null

      const response = await fetch('/api/user/login', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        credentials: 'same-origin',
        body: JSON.stringify({ username, password }),
      })

      if (!response.ok) {
        const errorData = await response.json().catch(() => ({ error: 'Login failed' }))
        throw new Error(errorData.error || 'Login failed')
      }

      // Fetch user info after successful login
      return await fetchUserInfo()
    } catch (err: any) {
      error.value = err.message || 'An error occurred during login'
      user.value = null
      return false
    } finally {
      isLoading.value = false
    }
  }

  const fetchUserInfo = async (): Promise<boolean> => {
    try {
      const response = await fetch('/api/user/me', {
        credentials: 'same-origin',
      })

      if (!response.ok) {
        // 401 is the expected "not logged in" case; don't log it as an error
        return false
      }

      user.value = await response.json()
      return true
    } catch (err: any) {
      console.error('Failed to fetch user info:', err)
      return false
    }
  }

  const logout = async () => {
    try {
      await fetch('/api/user/logout', {
        method: 'POST',
        credentials: 'same-origin',
      })
    } catch (err) {
      console.error('Failed to revoke session:', err)
    } finally {
      user.value = null
      error.value = null
    }
  }

  const clearError = () => {
    error.value = null
  }

  return {
    user: computed(() => user.value),
    isAuthenticated,
    isLoading: computed(() => isLoading.value),
    error: computed(() => error.value),
    login,
    logout,
    fetchUserInfo,
    clearError,
  }
}
