<script setup lang="ts">
import { ref, computed } from 'vue'
import { 
  X, 
  Maximize2, 
  Minimize2,
  Music,
  Video,
  Tag as TagIcon
} from 'lucide-vue-next'
import { tagService } from '../services/tag'
import { useAuth } from '../composables/useAuth'
import TagDialog from './TagDialog.vue'
import type { Storage, DirectoryEntry, Tag } from '../types'
import { onMounted } from 'vue'

const { user, token } = useAuth()

const props = defineProps<{
  storage: Storage
  entry: DirectoryEntry
  shareId?: number | string
}>()

const emit = defineEmits(['close'])

const isVideo = computed(() => {
  const ext = props.entry.path.split('.').pop()?.toLowerCase() || ''
  return ['mp4', 'webm', 'ogv', 'mov', 'mkv', 'avi', 'mpg', 'mpeg'].includes(ext)
})

const isAudio = computed(() => {
  const ext = props.entry.path.split('.').pop()?.toLowerCase() || ''
  return ['mp3', 'wav', 'ogg', 'm4a', 'aac', 'flac'].includes(ext)
})

const mediaUrl = computed(() => {
  let url = ''
  if (props.shareId !== undefined) {
    url = `/api/storage/share/${props.shareId}/show/${props.entry.path}`
  } else {
    url = `/api/storage/${props.storage.id}/show/${props.entry.path}`
  }
  
  if (token.value) {
    return `${url}?token=${token.value}`
  }
  return url
})

const isFullscreen = ref(false)

const toggleFullscreen = () => {
  isFullscreen.value = !isFullscreen.value
}

const getBasename = (path: string) => {
  return path.split('/').filter(s => s).pop() || path
}

const getThumbnailUrl = (entry: DirectoryEntry, size: 'small' | 'large' | 'mini') => {
  if (!entry.hash || entry.hash.length === 0) return null
  const hex = Array.from(entry.hash).map(b => b.toString(16).padStart(2, '0')).join('')
  return `/api/storage/thumbnail/${hex}/${size}`
}

// Tags logic
const allTags = ref<Tag[]>([])
const showTagsModal = ref(false)

const fetchTags = async () => {
  try {
    allTags.value = await tagService.getTags()
  } catch (err: any) {
    console.error('Failed to fetch tags:', err)
  }
}

const handleTagsUpdated = (newTags: number[]) => {
  props.entry.tags = newTags
}

onMounted(fetchTags)
</script>

<template>
  <div class="media-viewer-overlay" :class="{ fullscreen: isFullscreen }" @click.self="emit('close')">
    <div class="media-viewer-container glass-panel fade-in">
      <header class="viewer-header">
        <div class="file-info">
          <Video v-if="isVideo" :size="20" class="file-icon" />
          <Music v-else :size="20" class="file-icon" />
          <div class="file-meta">
            <h3>{{ getBasename(entry.path) }}</h3>
            <span class="path-label">{{ entry.path }}</span>
          </div>
        </div>

        <div class="header-tags" v-if="entry.tags?.length">
          <span v-for="tagId in entry.tags" :key="tagId" class="tag-pill">
            {{ allTags.find(t => t.id === tagId)?.name || 'Tag' }}
          </span>
        </div>

        <div class="viewer-actions">
          <button v-if="user?.admin" class="btn-icon" @click="showTagsModal = true" title="Manage Tags">
            <TagIcon :size="20" />
          </button>

          <button v-if="isVideo" class="btn-icon" @click="toggleFullscreen" :title="isFullscreen ? 'Exit Fullscreen' : 'Fullscreen'">
            <Minimize2 v-if="isFullscreen" :size="20" />
            <Maximize2 v-else :size="20" />
          </button>

          <button class="btn-icon close" @click="emit('close')" title="Close">
            <X :size="20" />
          </button>
        </div>
      </header>

      <div class="viewer-body">
        <div v-if="isVideo" class="video-wrapper">
          <video 
            ref="videoRef"
            :src="mediaUrl"
            class="video-player"
            controls
            autoplay
          >
            Your browser does not support the video tag.
          </video>
        </div>

        <div v-else-if="isAudio" class="audio-wrapper">
          <div class="audio-art glass-panel">
            <img v-if="entry.hash" :src="getThumbnailUrl(entry, 'large')!" class="album-art" @error="(e) => (e.target as HTMLImageElement).style.display = 'none'" />
            <Music :size="80" class="music-icon" />
          </div>
          <audio 
            ref="audioRef"
            :src="mediaUrl"
            class="audio-player"
            controls
            autoplay
          >
            Your browser does not support the audio tag.
          </audio>
        </div>
      </div>

      <footer class="viewer-footer">
        <div class="media-stats">
          <span>{{ isVideo ? 'Video' : 'Audio' }} Player</span>
          <span class="divider"></span>
          <span>{{ entry.size ? (entry.size / (1024 * 1024)).toFixed(2) + ' MB' : '' }}</span>
        </div>
      </footer>
    </div>

    <!-- Tags Dialog -->
    <TagDialog
      v-if="showTagsModal"
      :storage="storage"
      :entry="entry"
      @close="showTagsModal = false"
      @updated="handleTagsUpdated"
    />
  </div>
</template>

<style scoped>
.media-viewer-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.9);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 2001;
  padding: 40px;
  backdrop-filter: blur(12px);
  transition: all 0.3s ease;
}

.media-viewer-overlay.fullscreen {
  padding: 0;
}

.media-viewer-container {
  width: 100%;
  max-width: 1000px;
  max-height: 90vh;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: 0 25px 50px -12px rgba(0, 0, 0, 0.5);
  transition: all 0.3s cubic-bezier(0.4, 0, 0.2, 1);
}

.fullscreen .media-viewer-container {
  max-width: 100%;
  max-height: 100%;
  border-radius: 0;
}

.viewer-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 16px 24px;
  background: rgba(255, 255, 255, 0.03);
  border-bottom: 1px solid var(--border-color);
  z-index: 10;
}

.file-info {
  display: flex;
  align-items: center;
  gap: 16px;
}

.file-icon {
  color: var(--accent-color);
}

.file-meta h3 {
  font-size: 1.125rem;
  font-weight: 600;
  margin: 0;
}

.path-label {
  font-family: monospace;
}

.header-tags {
  display: flex;
  gap: 8px;
  margin-left: 20px;
  flex: 1;
  overflow: hidden;
}

.tag-pill {
  font-size: 0.7rem;
  padding: 2px 8px;
  background: rgba(59, 130, 246, 0.2);
  color: var(--accent-color);
  border-radius: 4px;
  white-space: nowrap;
  border: 1px solid rgba(59, 130, 246, 0.3);
}

.viewer-actions {
  display: flex;
  align-items: center;
  gap: 16px;
}

.btn-icon {
  padding: 8px;
  border-radius: 8px;
  background: none;
  border: none;
  color: var(--text-secondary);
  cursor: pointer;
  transition: all 0.2s;
}

.btn-icon:hover {
  color: var(--text-primary);
  background: rgba(255, 255, 255, 0.05);
}

.btn-icon.close:hover {
  color: #ef4444;
  background: rgba(239, 68, 68, 0.1);
}

.viewer-body {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  background: #000;
  position: relative;
  overflow: hidden;
}

.video-wrapper {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
}

.video-player {
  width: 100%;
  max-height: 100%;
  outline: none;
}

.audio-wrapper {
  width: 100%;
  max-width: 600px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 40px;
  padding: 40px;
}

.audio-art {
  width: 200px;
  height: 200px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 24px;
  background: rgba(255, 255, 255, 0.03);
}

.music-icon {
  color: var(--accent-color);
  opacity: 0.5;
  filter: drop-shadow(0 0 20px var(--accent-glow));
  position: absolute;
  z-index: 1;
}

.album-art {
  width: 100%;
  height: 100%;
  object-fit: cover;
  border-radius: 20px;
  z-index: 2;
  position: absolute;
}

.audio-player {
  width: 100%;
  height: 40px;
  border-radius: 8px;
  outline: none;
}

/* Customize audio player for modern look in Chrome/Webkit */
audio::-webkit-media-controls-enclosure {
  background-color: rgba(255, 255, 255, 0.8);
  backdrop-filter: blur(10px);
}

.viewer-footer {
  padding: 12px 24px;
  background: rgba(255, 255, 255, 0.02);
  border-top: 1px solid var(--border-color);
}

.media-stats {
  display: flex;
  align-items: center;
  gap: 12px;
  font-size: 0.875rem;
  color: var(--text-secondary);
}

.divider {
  width: 4px;
  height: 4px;
  background: var(--text-secondary);
  border-radius: 50%;
  opacity: 0.3;
}

.fade-in {
  animation: fadeIn 0.3s ease-out;
}

@keyframes fadeIn {
  from { opacity: 0; transform: translateY(10px); }
  to { opacity: 1; transform: translateY(0); }
}

/* Modal Inner styles */
.modal-overlay-inner {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.6);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
  backdrop-filter: blur(4px);
}

.modal {
  width: 90%;
  max-width: 400px;
}

.modal-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 16px 20px;
  border-bottom: 1px solid var(--border-color);
}

.modal-body {
  padding: 20px;
}

.tags-selection-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(110px, 1fr));
  gap: 8px;
  margin-bottom: 20px;
  max-height: 250px;
  overflow-y: auto;
}

.tag-checkbox-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px;
  cursor: pointer;
  transition: all 0.2s;
  font-size: 0.875rem;
}

.tag-checkbox-item.selected {
  background: rgba(59, 130, 246, 0.15);
  border-color: var(--accent-color);
  color: var(--accent-color);
}

.hidden-checkbox {
  display: none;
}

.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: 12px;
}

.btn-primary {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  background: var(--accent-color);
  border: none;
  border-radius: 8px;
  color: white;
  cursor: pointer;
}

.btn-secondary {
  padding: 8px 16px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  color: var(--text-primary);
  cursor: pointer;
}
</style>
