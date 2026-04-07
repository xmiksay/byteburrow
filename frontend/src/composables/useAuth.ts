import { ref, computed } from 'vue'

export interface User {
  id: number
  username: string
  name: string
  admin: boolean
}

export interface LoginResponse {
  token: string
  expires_in_days: number
}

const TOKEN_KEY = 'auth_token'
const token = ref<string | null>(localStorage.getItem(TOKEN_KEY))
const user = ref<User | null>(null)
const isLoading = ref(false)
const error = ref<string | null>(null)

export function useAuth() {
  const isAuthenticated = computed(() => !!token.value && !!user.value)

  const setToken = (newToken: string | null) => {
    token.value = newToken
    if (newToken) {
      localStorage.setItem(TOKEN_KEY, newToken)
    } else {
      localStorage.removeItem(TOKEN_KEY)
    }
  }

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

      const data: LoginResponse = await response.json()
      setToken(data.token)

      // Fetch user info after successful login
      await fetchUserInfo()

      return true
    } catch (err: any) {
      error.value = err.message || 'An error occurred during login'
      setToken(null)
      user.value = null
      return false
    } finally {
      isLoading.value = false
    }
  }

  const fetchUserInfo = async (): Promise<boolean> => {
    if (!token.value) return false

    try {
      const response = await fetch('/api/user/me', {
        headers: {
          'Authorization': `Bearer ${token.value}`,
        },
        credentials: 'same-origin',
      })

      if (!response.ok) {
        if (response.status === 401) {
          // Token is invalid or expired
          logout()
        }
        throw new Error('Failed to fetch user info')
      }

      user.value = await response.json()
      return true
    } catch (err: any) {
      console.error('Failed to fetch user info:', err)
      return false
    }
  }

  const logout = () => {
    setToken(null)
    user.value = null
    error.value = null
  }

  const clearError = () => {
    error.value = null
  }

  return {
    token: computed(() => token.value),
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
