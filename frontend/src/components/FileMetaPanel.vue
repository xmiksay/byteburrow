<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import { Info, Loader2, Tag as TagIcon, Brain, ChevronDown, ChevronRight } from 'lucide-vue-next'
import { storageService } from '../services/storage'
import { tagService } from '../services/tag'
import type { DirectoryEntry, MetaResponse, Tag } from '../types'
import { formatSize, formatDate } from '../utils/file'

const props = defineProps<{
  entry: DirectoryEntry
}>()

const meta = ref<MetaResponse | null>(null)
const allTags = ref<Tag[]>([])
const loading = ref(true)
const expandedSections = ref<Set<string>>(new Set(['basic', 'keywords', 'tags']))

const toggleSection = (section: string) => {
  if (expandedSections.value.has(section)) {
    expandedSections.value.delete(section)
  } else {
    expandedSections.value.add(section)
  }
}

const tagNames = computed(() => {
  if (!meta.value?.tags?.length) return []
  return meta.value.tags.map(id => allTags.value.find(t => t.id === id)?.name || `Tag #${id}`)
})

const customSections = computed(() => {
  if (!meta.value?.custom || typeof meta.value.custom !== 'object') return []
  return Object.entries(meta.value.custom).map(([key, value]) => ({
    key,
    label: key.replace(/_/g, ' ').replace(/\b\w/g, c => c.toUpperCase()),
    value,
  }))
})

const fetchMeta = async () => {
  if (!props.entry.hash || props.entry.hash.length === 0) {
    loading.value = false
    return
  }
  try {
    loading.value = true
    const [metaResult, tags] = await Promise.all([
      storageService.getMeta(props.entry.hash),
      tagService.getTags(),
    ])
    meta.value = metaResult
    allTags.value = tags
  } catch {
    // silently fail
  } finally {
    loading.value = false
  }
}

const formatCustomValue = (value: any): string => {
  if (typeof value === 'string') return value
  if (typeof value === 'number' || typeof value === 'boolean') return String(value)
  if (Array.isArray(value)) return value.join(', ')
  return JSON.stringify(value, null, 2)
}

const isSimpleValue = (value: any): boolean => {
  return typeof value !== 'object' || value === null
}

onMounted(fetchMeta)
</script>

<template>
  <div class="meta-panel">
    <div v-if="loading" class="meta-loading">
      <Loader2 :size="16" class="spin" />
      <span>Loading metadata...</span>
    </div>

    <template v-else>
      <!-- Basic File Info -->
      <div class="meta-section">
        <button class="section-header" @click="toggleSection('basic')">
          <component :is="expandedSections.has('basic') ? ChevronDown : ChevronRight" :size="14" />
          <span>File Info</span>
        </button>
        <div v-if="expandedSections.has('basic')" class="section-body">
          <div class="meta-row">
            <span class="meta-label">Size</span>
            <span class="meta-value">{{ formatSize(entry.size) }}</span>
          </div>
          <div class="meta-row">
            <span class="meta-label">Modified</span>
            <span class="meta-value">{{ formatDate(entry.modified_at) }}</span>
          </div>
          <div class="meta-row">
            <span class="meta-label">Created</span>
            <span class="meta-value">{{ formatDate(entry.created_at) }}</span>
          </div>
          <div class="meta-row" v-if="entry.hash">
            <span class="meta-label">Hash</span>
            <span class="meta-value mono">{{ Array.from(entry.hash).map(b => b.toString(16).padStart(2, '0')).join('').substring(0, 16) }}...</span>
          </div>
        </div>
      </div>

      <!-- Tags -->
      <div class="meta-section" v-if="tagNames.length">
        <button class="section-header" @click="toggleSection('tags')">
          <component :is="expandedSections.has('tags') ? ChevronDown : ChevronRight" :size="14" />
          <TagIcon :size="14" />
          <span>Tags</span>
          <span class="badge">{{ tagNames.length }}</span>
        </button>
        <div v-if="expandedSections.has('tags')" class="section-body">
          <div class="tags-list">
            <span v-for="name in tagNames" :key="name" class="tag-chip">{{ name }}</span>
          </div>
        </div>
      </div>

      <!-- Keywords -->
      <div class="meta-section" v-if="meta?.keywords?.length">
        <button class="section-header" @click="toggleSection('keywords')">
          <component :is="expandedSections.has('keywords') ? ChevronDown : ChevronRight" :size="14" />
          <Brain :size="14" />
          <span>Keywords</span>
          <span class="badge">{{ meta.keywords.length }}</span>
        </button>
        <div v-if="expandedSections.has('keywords')" class="section-body">
          <div class="tags-list">
            <span v-for="kw in meta.keywords" :key="kw" class="keyword-chip">{{ kw }}</span>
          </div>
        </div>
      </div>

      <!-- Custom Plugin Data -->
      <div class="meta-section" v-for="section in customSections" :key="section.key">
        <button class="section-header" @click="toggleSection(section.key)">
          <component :is="expandedSections.has(section.key) ? ChevronDown : ChevronRight" :size="14" />
          <span>{{ section.label }}</span>
        </button>
        <div v-if="expandedSections.has(section.key)" class="section-body">
          <template v-if="typeof section.value === 'object' && section.value !== null && !Array.isArray(section.value)">
            <div class="meta-row" v-for="[k, v] in Object.entries(section.value)" :key="k">
              <span class="meta-label">{{ k.replace(/_/g, ' ') }}</span>
              <span class="meta-value" :class="{ mono: !isSimpleValue(v) }">
                {{ formatCustomValue(v) }}
              </span>
            </div>
          </template>
          <template v-else>
            <div class="meta-row">
              <span class="meta-value mono">{{ formatCustomValue(section.value) }}</span>
            </div>
          </template>
        </div>
      </div>

      <!-- No metadata -->
      <div v-if="!meta && !loading" class="meta-empty">
        <Info :size="16" />
        <span>No metadata available yet</span>
      </div>
    </template>
  </div>
</template>

<style scoped>
.meta-panel {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 8px 0;
  font-size: 0.8125rem;
}

.meta-loading {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 16px;
  color: var(--text-secondary);
}

.meta-section {
  border-bottom: 1px solid rgba(255, 255, 255, 0.04);
}

.section-header {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 10px 16px;
  background: none;
  border: none;
  color: var(--text-primary);
  font-size: 0.8125rem;
  font-weight: 600;
  cursor: pointer;
  transition: background 0.15s;
}

.section-header:hover {
  background: rgba(255, 255, 255, 0.03);
}

.badge {
  margin-left: auto;
  font-size: 0.6875rem;
  font-weight: 500;
  padding: 1px 6px;
  background: rgba(59, 130, 246, 0.15);
  color: var(--accent-color);
  border-radius: 8px;
}

.section-body {
  padding: 4px 16px 12px 38px;
}

.meta-row {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
  padding: 3px 0;
  gap: 12px;
}

.meta-label {
  color: var(--text-secondary);
  font-size: 0.75rem;
  text-transform: capitalize;
  flex-shrink: 0;
}

.meta-value {
  color: var(--text-primary);
  text-align: right;
  word-break: break-all;
}

.meta-value.mono {
  font-family: 'JetBrains Mono', 'Fira Code', monospace;
  font-size: 0.75rem;
}

.tags-list {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.tag-chip {
  font-size: 0.7rem;
  padding: 2px 8px;
  background: rgba(59, 130, 246, 0.15);
  color: var(--accent-color);
  border-radius: 4px;
  border: 1px solid rgba(59, 130, 246, 0.25);
}

.keyword-chip {
  font-size: 0.7rem;
  padding: 2px 8px;
  background: rgba(16, 185, 129, 0.15);
  color: #10b981;
  border-radius: 4px;
  border: 1px solid rgba(16, 185, 129, 0.25);
}

.meta-empty {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 16px;
  color: var(--text-secondary);
  font-size: 0.8125rem;
}

.spin {
  animation: spin 1s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}
</style>
