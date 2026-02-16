<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { Camera, ChevronLeft, ChevronRight, Calendar, MapPin, X, RefreshCw } from 'lucide-vue-next'
import { photoService } from '../services/photo'
import { useAuth } from '../composables/useAuth'
import type { Photo } from '../types'
import { getBasename } from '../utils/file'

const { user } = useAuth()

const route = useRoute()
const router = useRouter()

const photos = ref<Photo[]>([])
const loading = ref(false)
const error = ref<string | null>(null)

// Date navigation — synced with route params
const now = new Date()
const selectedYear = ref<number | null>(null)
const selectedMonth = ref<number | null>(null)
const selectedDay = ref<number | null>(null)

const initFromRoute = () => {
  const y = route.params.year ? parseInt(route.params.year as string) : null
  const m = route.params.month ? parseInt(route.params.month as string) : null
  const d = route.params.day ? parseInt(route.params.day as string) : null
  selectedYear.value = y && !isNaN(y) ? y : null
  selectedMonth.value = m && !isNaN(m) ? m : null
  selectedDay.value = d && !isNaN(d) ? d : null
}

// Default to current year/month when navigating to /photos with no params
if (!route.params.year) {
  selectedYear.value = now.getFullYear()
  selectedMonth.value = now.getMonth() + 1
} else {
  initFromRoute()
}

const pushRoute = () => {
  const params: Record<string, string> = {}
  if (selectedYear.value !== null) {
    params.year = String(selectedYear.value)
    if (selectedMonth.value !== null) {
      params.month = String(selectedMonth.value)
      if (selectedDay.value !== null) {
        params.day = String(selectedDay.value)
      }
    }
  }
  router.replace({ name: 'photos', params })
}

// Lightbox
const lightboxPhoto = ref<Photo | null>(null)

const dateLabel = computed(() => {
  if (selectedYear.value === null) return 'All Photos'
  if (selectedDay.value !== null && selectedMonth.value !== null) {
    const d = new Date(selectedYear.value, selectedMonth.value - 1, selectedDay.value)
    return d.toLocaleDateString('en-US', { year: 'numeric', month: 'long', day: 'numeric' })
  }
  if (selectedMonth.value !== null) {
    const d = new Date(selectedYear.value, selectedMonth.value - 1)
    return d.toLocaleDateString('en-US', { year: 'numeric', month: 'long' })
  }
  return String(selectedYear.value)
})

const fetchPhotos = async () => {
  try {
    loading.value = true
    error.value = null
    if (selectedYear.value === null) {
      photos.value = await photoService.listAll()
    } else if (selectedDay.value !== null && selectedMonth.value !== null) {
      photos.value = await photoService.listByDay(selectedYear.value, selectedMonth.value, selectedDay.value)
    } else if (selectedMonth.value !== null) {
      photos.value = await photoService.listByMonth(selectedYear.value, selectedMonth.value)
    } else {
      photos.value = await photoService.listByYear(selectedYear.value)
    }
  } catch (err: any) {
    error.value = 'Failed to load photos: ' + err.message
  } finally {
    loading.value = false
  }
}

const thumbnailUrl = (photo: Photo, size: string = 'small') => {
  return `/api/storage/thumbnail/${photo.hash}/${size}`
}

const originalUrl = (photo: Photo) => {
  if (!photo.storage_id || !photo.path) return null
  return `/api/storage/${photo.storage_id}/show/${photo.path}`
}

const prevYear = () => {
  if (selectedYear.value === null) return
  selectedYear.value--
  selectedMonth.value = null
  selectedDay.value = null
  pushRoute()
}

const nextYear = () => {
  if (selectedYear.value === null) return
  selectedYear.value++
  selectedMonth.value = null
  selectedDay.value = null
  pushRoute()
}

const showAll = () => {
  selectedYear.value = null
  selectedMonth.value = null
  selectedDay.value = null
  pushRoute()
}

const clearYear = () => {
  showAll()
}

const selectYear = (year: number) => {
  selectedYear.value = year
  selectedMonth.value = null
  selectedDay.value = null
  pushRoute()
}

const selectMonth = (month: number) => {
  selectedMonth.value = month
  selectedDay.value = null
  pushRoute()
}

const clearMonth = () => {
  selectedMonth.value = null
  selectedDay.value = null
  pushRoute()
}

const clearDay = () => {
  selectedDay.value = null
  pushRoute()
}

// HTML5 date picker sync
const dateInputValue = computed(() => {
  if (selectedYear.value === null) return ''
  const y = String(selectedYear.value).padStart(4, '0')
  if (selectedMonth.value === null) return `${y}-01-01`
  const m = String(selectedMonth.value).padStart(2, '0')
  if (selectedDay.value === null) return `${y}-${m}-01`
  const d = String(selectedDay.value).padStart(2, '0')
  return `${y}-${m}-${d}`
})

const onDateInput = (e: Event) => {
  const val = (e.target as HTMLInputElement).value
  if (!val) {
    showAll()
    return
  }
  const parts = val.split('-')
  selectedYear.value = parseInt(parts[0])
  selectedMonth.value = parseInt(parts[1])
  selectedDay.value = parseInt(parts[2])
  pushRoute()
}

const openLightbox = (photo: Photo) => {
  lightboxPhoto.value = photo
}

const closeLightbox = () => {
  lightboxPhoto.value = null
}

const regenerating = ref(false)

const regenerateThumbnail = async () => {
  if (!lightboxPhoto.value || regenerating.value) return
  regenerating.value = true
  try {
    await photoService.regenerateThumbnail(lightboxPhoto.value.hash)
  } catch (err: any) {
    console.error('Failed to regenerate thumbnail:', err)
  } finally {
    regenerating.value = false
  }
}

// Flat list of all photos for prev/next navigation
const allPhotos = computed(() => {
  const result: Photo[] = []
  for (const group of photosByDate.value) {
    result.push(...group.photos)
  }
  return result
})

const lightboxIndex = computed(() => {
  if (!lightboxPhoto.value) return -1
  return allPhotos.value.findIndex(p => p.hash === lightboxPhoto.value!.hash)
})

const prevPhoto = () => {
  const idx = lightboxIndex.value
  if (idx > 0) {
    lightboxPhoto.value = allPhotos.value[idx - 1]
  }
}

const nextPhoto = () => {
  const idx = lightboxIndex.value
  if (idx >= 0 && idx < allPhotos.value.length - 1) {
    lightboxPhoto.value = allPhotos.value[idx + 1]
  }
}

const onKeydown = (e: KeyboardEvent) => {
  if (!lightboxPhoto.value) return
  if (e.key === 'ArrowLeft') prevPhoto()
  else if (e.key === 'ArrowRight') nextPhoto()
  else if (e.key === 'Escape') closeLightbox()
}

// Group photos by date for display
const photosByDate = computed(() => {
  const groups: { date: string; photos: Photo[] }[] = []
  const map = new Map<string, Photo[]>()

  for (const p of photos.value) {
    const key = p.date ? p.date.split(' ')[0] : 'Unknown'
    if (!map.has(key)) {
      map.set(key, [])
    }
    map.get(key)!.push(p)
  }

  for (const [date, items] of map) {
    groups.push({ date, photos: items })
  }

  return groups
})

// Available years from current photos (for year picker when showing all)
const availableYears = computed(() => {
  const years = new Set<number>()
  for (const p of photos.value) {
    if (p.date) {
      const parts = p.date.split('-')
      if (parts.length >= 1) {
        years.add(parseInt(parts[0]))
      }
    }
  }
  return Array.from(years).sort((a, b) => b - a)
})

// Available months from current photos (for month picker)
const availableMonths = computed(() => {
  const months = new Set<number>()
  for (const p of photos.value) {
    if (p.date) {
      const parts = p.date.split('-')
      if (parts.length >= 2) {
        months.add(parseInt(parts[1]))
      }
    }
  }
  return Array.from(months).sort((a, b) => a - b)
})

const monthName = (m: number) => {
  return new Date(2000, m - 1).toLocaleDateString('en-US', { month: 'short' })
}

const formatDate = (dateStr: string) => {
  if (dateStr === 'Unknown') return dateStr
  const d = new Date(dateStr)
  return d.toLocaleDateString('en-US', { weekday: 'short', month: 'short', day: 'numeric' })
}

watch([selectedYear, selectedMonth, selectedDay], () => {
  fetchPhotos()
})

// Sync state when navigating back/forward
watch(() => route.params, () => {
  if (route.name !== 'photos') return
  initFromRoute()
})

onMounted(() => {
  fetchPhotos()
  window.addEventListener('keydown', onKeydown)
})

onUnmounted(() => {
  window.removeEventListener('keydown', onKeydown)
})
</script>

<template>
  <div class="photo-library">
    <div class="library-header">
      <div class="date-nav glass-panel">
        <button class="btn-icon" @click="prevYear" :disabled="selectedYear === null" title="Previous Year">
          <ChevronLeft :size="18" />
        </button>

        <div class="date-breadcrumbs">
          <span class="date-segment" :class="{ clickable: selectedYear !== null }" @click="clearYear">
            All
          </span>
          <template v-if="selectedYear !== null">
            <ChevronRight :size="14" class="date-sep" />
            <span class="date-segment" :class="{ clickable: selectedMonth !== null }" @click="clearMonth">
              {{ selectedYear }}
            </span>
          </template>
          <template v-if="selectedMonth !== null">
            <ChevronRight :size="14" class="date-sep" />
            <span class="date-segment" :class="{ clickable: selectedDay !== null }" @click="clearDay">
              {{ monthName(selectedMonth) }}
            </span>
          </template>
          <template v-if="selectedDay !== null">
            <ChevronRight :size="14" class="date-sep" />
            <span class="date-segment">{{ selectedDay }}</span>
          </template>
        </div>

        <button class="btn-icon" @click="nextYear" :disabled="selectedYear === null" title="Next Year">
          <ChevronRight :size="18" />
        </button>

        <label class="date-picker-btn" title="Pick a date">
          <Calendar :size="16" />
          <input type="date" :value="dateInputValue" @input="onDateInput" class="date-input" />
        </label>
      </div>

      <div v-if="selectedYear === null && availableYears.length > 0" class="month-picker glass-panel">
        <button
          v-for="y in availableYears"
          :key="y"
          class="month-btn"
          @click="selectYear(y)"
        >
          {{ y }}
        </button>
      </div>

      <div v-if="selectedYear !== null && selectedMonth === null && availableMonths.length > 0" class="month-picker glass-panel">
        <button
          v-for="m in availableMonths"
          :key="m"
          class="month-btn"
          @click="selectMonth(m)"
        >
          {{ monthName(m) }}
        </button>
      </div>
    </div>

    <div v-if="error" class="error-state glass-panel fade-in">
      <p>{{ error }}</p>
      <button @click="fetchPhotos" class="btn-primary">Retry</button>
    </div>

    <div v-else-if="loading" class="loading-state">
      <div class="spinner"></div>
    </div>

    <div v-else-if="photos.length === 0" class="empty-state glass-panel fade-in">
      <Camera :size="48" style="opacity: 0.2" />
      <p>No photos for {{ dateLabel }}</p>
    </div>

    <div v-else class="photo-content fade-in">
      <div v-for="group in photosByDate" :key="group.date" class="date-group">
        <div class="date-group-header">
          <Calendar :size="14" />
          <span>{{ formatDate(group.date) }}</span>
          <span class="photo-count">{{ group.photos.length }}</span>
        </div>
        <div class="photo-grid">
          <div
            v-for="photo in group.photos"
            :key="photo.hash"
            class="photo-card"
            @click="openLightbox(photo)"
          >
            <img
              :src="thumbnailUrl(photo)"
              :alt="photo.path ? getBasename(photo.path) : photo.hash"
              loading="lazy"
              @error="(e) => (e.target as HTMLImageElement).style.display = 'none'"
            />
            <div v-if="photo.latitude && photo.longitude" class="photo-location">
              <MapPin :size="10" />
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- Lightbox -->
    <div v-if="lightboxPhoto" class="lightbox-overlay" @click.self="closeLightbox">
      <div class="lightbox-top-actions">
        <button v-if="user?.admin" class="lightbox-action" :class="{ spinning: regenerating }" @click="regenerateThumbnail" title="Regenerate Thumbnail">
          <RefreshCw :size="20" />
        </button>
        <button class="lightbox-close" @click="closeLightbox">
          <X :size="24" />
        </button>
      </div>
      <button v-if="lightboxIndex > 0" class="lightbox-nav lightbox-prev" @click="prevPhoto">
        <ChevronLeft :size="28" />
      </button>
      <button v-if="lightboxIndex < allPhotos.length - 1" class="lightbox-nav lightbox-next" @click="nextPhoto">
        <ChevronRight :size="28" />
      </button>
      <div class="lightbox-content">
        <img
          v-if="originalUrl(lightboxPhoto)"
          :src="originalUrl(lightboxPhoto)!"
          :alt="lightboxPhoto.path ? getBasename(lightboxPhoto.path) : lightboxPhoto.hash"
        />
        <img
          v-else
          :src="thumbnailUrl(lightboxPhoto, 'large')"
          :alt="lightboxPhoto.hash"
        />
      </div>
      <div class="lightbox-info glass-panel">
        <p v-if="lightboxPhoto.path" class="lightbox-name">{{ getBasename(lightboxPhoto.path) }}</p>
        <p v-if="lightboxPhoto.date" class="lightbox-date">{{ lightboxPhoto.date }}</p>
        <p v-if="lightboxPhoto.latitude && lightboxPhoto.longitude" class="lightbox-gps">
          <MapPin :size="12" />
          {{ lightboxPhoto.latitude.toFixed(4) }}, {{ lightboxPhoto.longitude.toFixed(4) }}
        </p>
        <div v-if="lightboxPhoto.keywords.length > 0" class="lightbox-keywords">
          <span v-for="kw in lightboxPhoto.keywords" :key="kw" class="keyword-pill">{{ kw }}</span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.photo-library {
  display: flex;
  flex-direction: column;
  gap: 20px;
  height: 100%;
}

.library-header {
  display: flex;
  gap: 12px;
  align-items: center;
  flex-wrap: wrap;
}

.date-nav {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
}

.date-breadcrumbs {
  display: flex;
  align-items: center;
  gap: 4px;
  font-weight: 600;
  font-size: 0.9rem;
}

.date-segment {
  color: var(--text-primary);
}

.date-segment.clickable {
  color: var(--text-secondary);
  cursor: pointer;
  transition: color 0.2s;
}

.date-segment.clickable:hover {
  color: var(--accent-color);
}

.date-sep {
  color: var(--text-secondary);
  opacity: 0.5;
}

.btn-icon {
  padding: 6px;
  border-radius: 6px;
  background: transparent;
  border: none;
  color: var(--text-secondary);
  cursor: pointer;
  transition: all 0.2s;
  display: flex;
  align-items: center;
}

.btn-icon:hover:not(:disabled) {
  color: var(--text-primary);
  background: rgba(255, 255, 255, 0.05);
}

.btn-icon:disabled {
  opacity: 0.3;
  cursor: default;
}

.date-picker-btn {
  display: flex;
  align-items: center;
  padding: 4px 8px;
  border-left: 1px solid var(--border-color);
  margin-left: 4px;
  color: var(--text-secondary);
  cursor: pointer;
  position: relative;
}

.date-picker-btn:hover {
  color: var(--accent-color);
}

.date-input {
  position: absolute;
  inset: 0;
  opacity: 0;
  width: 100%;
  cursor: pointer;
}

.month-picker {
  display: flex;
  gap: 4px;
  padding: 6px 12px;
  flex-wrap: wrap;
}

.month-btn {
  padding: 4px 10px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: rgba(255, 255, 255, 0.03);
  color: var(--text-secondary);
  font-size: 0.75rem;
  cursor: pointer;
  transition: all 0.2s;
}

.month-btn:hover {
  background: rgba(59, 130, 246, 0.1);
  color: var(--accent-color);
  border-color: rgba(59, 130, 246, 0.3);
}

.loading-state {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 80px;
}

.spinner {
  width: 32px;
  height: 32px;
  border: 3px solid rgba(255, 255, 255, 0.1);
  border-top-color: var(--accent-color);
  border-radius: 50%;
  animation: spin 1s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}

.empty-state, .error-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 80px;
  gap: 16px;
  color: var(--text-secondary);
}

.photo-content {
  display: flex;
  flex-direction: column;
  gap: 24px;
  overflow-y: auto;
  flex: 1;
  scrollbar-width: none;
}

.photo-content::-webkit-scrollbar {
  display: none;
}

.date-group-header {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 0.8rem;
  font-weight: 600;
  color: var(--text-secondary);
  padding: 0 4px;
  margin-bottom: 8px;
}

.photo-count {
  font-size: 0.7rem;
  padding: 1px 6px;
  background: rgba(255, 255, 255, 0.05);
  border-radius: 8px;
  font-weight: 400;
}

.photo-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
  gap: 8px;
}

.photo-card {
  aspect-ratio: 1;
  border-radius: 8px;
  overflow: hidden;
  cursor: pointer;
  position: relative;
  background: rgba(255, 255, 255, 0.03);
  border: 1px solid var(--border-color);
  transition: all 0.2s;
}

.photo-card:hover {
  border-color: var(--accent-color);
  transform: scale(1.02);
}

.photo-card img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.photo-location {
  position: absolute;
  bottom: 4px;
  right: 4px;
  padding: 2px 4px;
  background: rgba(0, 0, 0, 0.6);
  border-radius: 4px;
  color: var(--accent-color);
  display: flex;
  align-items: center;
}

/* Lightbox */
.lightbox-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.9);
  z-index: 4000;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 40px;
}

.lightbox-top-actions {
  position: absolute;
  top: 16px;
  right: 16px;
  display: flex;
  gap: 8px;
  z-index: 4001;
}

.lightbox-close,
.lightbox-action {
  padding: 8px;
  background: rgba(255, 255, 255, 0.1);
  border: none;
  border-radius: 8px;
  color: var(--text-primary);
  cursor: pointer;
  transition: all 0.2s;
  display: flex;
  align-items: center;
}

.lightbox-close:hover,
.lightbox-action:hover {
  background: rgba(255, 255, 255, 0.2);
}

.lightbox-action.spinning {
  animation: spin 1s linear infinite;
}

.lightbox-nav {
  position: absolute;
  top: 50%;
  transform: translateY(-50%);
  padding: 12px;
  background: rgba(255, 255, 255, 0.1);
  border: none;
  border-radius: 50%;
  color: var(--text-primary);
  cursor: pointer;
  z-index: 4001;
  transition: background 0.2s;
  display: flex;
  align-items: center;
}

.lightbox-nav:hover {
  background: rgba(255, 255, 255, 0.2);
}

.lightbox-prev {
  left: 16px;
}

.lightbox-next {
  right: 16px;
}

.lightbox-content {
  max-width: 90vw;
  max-height: 80vh;
  display: flex;
  align-items: center;
  justify-content: center;
}

.lightbox-content img {
  max-width: 100%;
  max-height: 80vh;
  object-fit: contain;
  border-radius: 8px;
}

.lightbox-info {
  position: absolute;
  bottom: 20px;
  left: 50%;
  transform: translateX(-50%);
  padding: 12px 20px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 0.8rem;
  max-width: 600px;
}

.lightbox-name {
  font-weight: 600;
  color: var(--text-primary);
}

.lightbox-date {
  color: var(--text-secondary);
}

.lightbox-gps {
  display: flex;
  align-items: center;
  gap: 4px;
  color: var(--accent-color);
  font-family: 'JetBrains Mono', monospace;
  font-size: 0.75rem;
}

.lightbox-keywords {
  display: flex;
  gap: 4px;
  flex-wrap: wrap;
  margin-top: 4px;
}

.keyword-pill {
  font-size: 0.65rem;
  padding: 2px 6px;
  background: rgba(59, 130, 246, 0.15);
  color: var(--accent-color);
  border-radius: 4px;
  border: 1px solid rgba(59, 130, 246, 0.2);
}

.fade-in {
  animation: fadeIn 0.3s ease-out;
}

@keyframes fadeIn {
  from { opacity: 0; transform: translateY(5px); }
  to { opacity: 1; transform: translateY(0); }
}

@media (max-width: 600px) {
  .photo-grid {
    grid-template-columns: repeat(auto-fill, minmax(100px, 1fr));
    gap: 4px;
  }

  .lightbox-overlay {
    padding: 10px;
  }
}
</style>
