<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import { 
  X, 
  Save, 
  Eye, 
  Edit3, 
  Loader2,
  FileText,
  AlertCircle,
  Maximize2,
  Minimize2,
  Tag as TagIcon
} from 'lucide-vue-next'
import { marked } from 'marked'
import { markedHighlight } from 'marked-highlight'
import hljs from 'highlight.js'
import 'highlight.js/styles/github-dark.css'
import { storageService } from '../services/storage'
import { tagService } from '../services/tag'
import { useAuth } from '../composables/useAuth'
import TagDialog from './TagDialog.vue'
import type { Storage, DirectoryEntry, Tag } from '../types'

const { user } = useAuth()

// Configure marked to use highlight.js
marked.use(markedHighlight({
  langPrefix: 'hljs language-',
  highlight(code: string, lang: string) {
    const language = hljs.getLanguage(lang) ? lang : 'plaintext'
    return hljs.highlight(code, { language }).value
  }
}))

const props = defineProps<{
  storage: Storage
  entry: DirectoryEntry
  shareId?: number  // If provided, use share-based access
  readOnly?: boolean  // Force read-only mode (e.g., share without write permission)
}>()

const emit = defineEmits(['close'])

const content = ref('')
const originalContent = ref('')
const mode = ref<'preview' | 'edit'>('preview')
const loading = ref(true)
const saving = ref(false)
const error = ref<string | null>(null)
const isFullscreen = ref(false)

const canEdit = computed(() => !props.readOnly)

const isMarkdown = computed(() => {
  return props.entry.path.toLowerCase().endsWith('.md') || props.entry.path.toLowerCase().endsWith('.markdown')
})

const highlightedContent = computed(() => {
  if (!content.value) return ''
  
  if (isMarkdown.value) {
    return marked.parse(content.value)
  }
  
  // For other code files, highlight based on extension or auto-detect
  const ext = props.entry.path.split('.').pop()?.toLowerCase() || ''
  const lang = hljs.getLanguage(ext) ? ext : undefined
  
  const result = lang 
    ? hljs.highlight(content.value, { language: lang })
    : hljs.highlightAuto(content.value)
    
  return `<pre><code class="hljs language-${result.language || 'plaintext'}">${result.value}</code></pre>`
})

const isDirty = computed(() => {
  return content.value !== originalContent.value
})

const fetchContent = async () => {
  try {
    loading.value = true
    error.value = null
    let text: string
    if (props.shareId !== undefined) {
      text = await storageService.getShareFileContent(props.shareId, props.entry.path)
    } else {
      text = await storageService.getFileContent(props.storage.id, props.entry.path)
    }
    content.value = text
    originalContent.value = text
    // Default to preview mode for all text files if they have content
    mode.value = text.length > 0 ? 'preview' : 'edit'
  } catch (err: any) {
    error.value = 'Failed to load file content: ' + err.message
  } finally {
    loading.value = false
  }
}

const handleSave = async () => {
  if (!canEdit.value) return
  try {
    saving.value = true
    error.value = null
    if (props.shareId !== undefined) {
      await storageService.updateShareFileContent(props.shareId, props.entry.path, content.value)
    } else {
      await storageService.updateFileContent(props.storage.id, props.entry.path, content.value)
    }
    originalContent.value = content.value
  } catch (err: any) {
    error.value = 'Failed to save file: ' + err.message
  } finally {
    saving.value = false
  }
}

const toggleFullscreen = () => {
  isFullscreen.value = !isFullscreen.value
}

onMounted(() => {
  fetchContent()
})

const getBasename = (path: string) => {
  return path.split('/').filter(s => s).pop() || path
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
  <div class="file-viewer-overlay" :class="{ fullscreen: isFullscreen }" @click.self="emit('close')">
    <div class="file-viewer-container glass-panel fade-in">
      <header class="viewer-header">
        <div class="file-info">
          <FileText :size="20" class="file-icon" />
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
          <div class="mode-switcher glass-panel">
            <button 
              class="mode-btn" 
              :class="{ active: mode === 'preview' }"
              @click="mode = 'preview'"
            >
              <Eye :size="16" />
              <span>Preview</span>
            </button>
            <button 
              v-if="canEdit"
              class="mode-btn" 
              :class="{ active: mode === 'edit' }"
              @click="mode = 'edit'"
            >
              <Edit3 :size="16" />
              <span>Edit</span>
            </button>
          </div>

          <button v-if="canEdit" class="btn-primary" :disabled="!isDirty || saving" @click="handleSave">
            <Loader2 v-if="saving" :size="16" class="spin" />
            <Save v-else :size="16" />
            <span>{{ saving ? 'Saving...' : 'Save' }}</span>
          </button>

          <button v-if="user?.admin" class="btn-icon" @click="showTagsModal = true" title="Manage Tags">
            <TagIcon :size="20" />
          </button>

          <div class="header-divider"></div>

          <button class="btn-icon" @click="toggleFullscreen" :title="isFullscreen ? 'Exit Fullscreen' : 'Fullscreen'">
            <Minimize2 v-if="isFullscreen" :size="20" />
            <Maximize2 v-else :size="20" />
          </button>

          <button class="btn-icon close" @click="emit('close')" title="Close">
            <X :size="20" />
          </button>
        </div>
      </header>

      <div class="viewer-body">
        <div v-if="loading" class="body-loading">
          <div class="spinner"></div>
          <p>Loading content...</p>
        </div>

        <div v-else-if="error" class="body-error">
          <AlertCircle :size="48" class="error-icon" />
          <p>{{ error }}</p>
          <button class="btn-secondary" @click="fetchContent">Retry</button>
        </div>

        <template v-else>
          <!-- Preview Mode -->
          <div v-if="mode === 'preview'" 
               class="content-preview" 
               :class="{ 'markdown-body': isMarkdown, 'code-preview': !isMarkdown }" 
               v-html="highlightedContent">
          </div>

          <!-- Edit Mode -->
          <div v-else class="editor-container">
            <textarea 
              v-model="content" 
              class="content-editor" 
              placeholder="Start typing..."
              spellcheck="false"
            ></textarea>
          </div>
        </template>
      </div>

      <footer class="viewer-footer">
        <div class="status-info">
          <span v-if="isDirty" class="dirty-indicator">Unsaved changes</span>
          <span v-else class="saved-indicator">All changes saved</span>
        </div>
        <div class="editor-stats">
          <span>{{ content.length }} characters</span>
          <span>{{ content.split(/\s+/).filter(s => s).length }} words</span>
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
.file-viewer-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.8);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 2000;
  padding: 40px;
  backdrop-filter: blur(8px);
  transition: all 0.3s ease;
}

.file-viewer-overlay.fullscreen {
  padding: 0;
}

.file-viewer-container {
  width: 100%;
  max-width: 1200px;
  height: 100%;
  max-height: 90vh;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: 0 25px 50px -12px rgba(0, 0, 0, 0.5);
  transition: all 0.3s cubic-bezier(0.4, 0, 0.2, 1);
}

.fullscreen .file-viewer-container {
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

.mode-switcher {
  display: flex;
  padding: 4px;
  border-radius: 8px;
}

.mode-btn {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 12px;
  border-radius: 6px;
  background: none;
  border: none;
  color: var(--text-secondary);
  cursor: pointer;
  font-size: 0.875rem;
  font-weight: 500;
  transition: all 0.2s;
}

.mode-btn:hover {
  color: var(--text-primary);
  background: rgba(255, 255, 255, 0.05);
}

.mode-btn.active {
  background: rgba(59, 130, 246, 0.1);
  color: var(--accent-color);
}

.header-divider {
  width: 1px;
  height: 24px;
  background: var(--border-color);
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
  font-weight: 500;
  transition: all 0.2s;
}

.btn-primary:disabled {
  opacity: 0.5;
  cursor: not-allowed;
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
  overflow: hidden;
  position: relative;
  background: rgba(0, 0, 0, 0.2);
}

.body-loading, .body-error {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
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

.error-icon {
  color: #ef4444;
}

.editor-container {
  height: 100%;
  padding: 0;
}

.content-editor {
  width: 100%;
  height: 100%;
  background: none;
  border: none;
  color: #e0e0e0;
  font-family: 'JetBrains Mono', 'Fira Code', monospace;
  font-size: 1rem;
  line-height: 1.6;
  padding: 32px;
  outline: none;
  resize: none;
}

.markdown-body, .code-preview {
  height: 100%;
  overflow-y: auto;
  padding: 40px;
  color: #e0e0e0;
  line-height: 1.6;
}

.code-preview :deep(pre) {
  margin: 0;
  padding: 0;
  background: none;
}

.code-preview :deep(.hljs) {
  background: none;
  padding: 0;
  font-family: 'JetBrains Mono', 'Fira Code', monospace;
  font-size: 0.95rem;
}

/* Basic Markdown Styling */
.markdown-preview :deep(h1), .markdown-preview :deep(h2), .markdown-preview :deep(h3) {
  margin-top: 24px;
  margin-bottom: 16px;
  font-weight: 600;
  line-height: 1.25;
}

.markdown-preview :deep(h1) { font-size: 2rem; border-bottom: 1px solid var(--border-color); padding-bottom: 0.3em; }
.markdown-preview :deep(h2) { font-size: 1.5rem; border-bottom: 1px solid var(--border-color); padding-bottom: 0.3em; }

.markdown-preview :deep(p) { margin-bottom: 16px; }

.markdown-preview :deep(code) {
  padding: 0.2em 0.4em;
  margin: 0;
  font-size: 85%;
  background-color: rgba(255, 255, 255, 0.1);
  border-radius: 6px;
  font-family: monospace;
}

.markdown-preview :deep(pre) {
  padding: 16px;
  overflow: auto;
  font-size: 85%;
  line-height: 1.45;
  background-color: rgba(0, 0, 0, 0.3);
  border-radius: 12px;
  margin-bottom: 16px;
}

.markdown-preview :deep(pre code) {
  padding: 0;
  background: none;
}

.viewer-footer {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 8px 24px;
  background: rgba(255, 255, 255, 0.02);
  border-top: 1px solid var(--border-color);
  font-size: 0.75rem;
  color: var(--text-secondary);
}

.dirty-indicator { color: #f59e0b; }
.saved-indicator { color: #10b981; }

.editor-stats {
  display: flex;
  gap: 16px;
}

@keyframes spin {
  to { transform: rotate(360deg); }
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
  padding: 16px 20px;
  border-bottom: 1px solid var(--border-color);
  background: none;
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

.btn-secondary {
  padding: 8px 16px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--border-color);
  border-radius: 8px;
  color: var(--text-primary);
  cursor: pointer;
}

.spin {
  animation: spin 1s linear infinite;
}
</style>
