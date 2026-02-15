import type { DirectoryEntry } from '../types'

/**
 * Format file size in human-readable format
 */
export const formatSize = (bytes?: number | null): string => {
    if (bytes === undefined || bytes === null) return ''
    const units = ['B', 'KB', 'MB', 'GB', 'TB']
    let size = bytes
    let unitIdx = 0
    while (size >= 1024 && unitIdx < units.length - 1) {
        size /= 1024
        unitIdx++
    }
    return unitIdx === 0 ? `${bytes} B` : `${size.toFixed(0)} ${units[unitIdx]}`
}

/**
 * Format date in short locale format
 */
export const formatDate = (dateStr?: string): string => {
    if (!dateStr) return '-'
    const date = new Date(dateStr)
    return date.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: '2-digit' })
}

/**
 * Get basename of a path
 */
export const getBasename = (path: string): string => {
    return path.split('/').filter(s => s).pop() || path
}

/**
 * Check if file is viewable as text
 */
export const isViewableAsText = (path: string): boolean => {
    const ext = path.split('.').pop()?.toLowerCase()
    return [
        'txt', 'md', 'markdown', 'rs', 'js', 'mjs', 'ts', 'tsx', 'jsx', 'json',
        'css', 'html', 'htm', 'toml', 'yaml', 'yml', 'xml', 'csv', 'ini',
        'py', 'java', 'c', 'cpp', 'cc', 'cxx', 'h', 'hpp', 'go', 'sh', 'rb',
        'php', 'swift', 'kt', 'kts', 'vue', 'sql'
    ].includes(ext || '')
}

/**
 * Check if file is an image
 */
export const isImage = (path: string): boolean => {
    const ext = path.split('.').pop()?.toLowerCase() || ''
    return ['jpg', 'jpeg', 'png', 'gif', 'webp', 'bmp', 'svg'].includes(ext)
}

/**
 * Check if file is a video
 */
export const isVideo = (path: string): boolean => {
    const ext = path.split('.').pop()?.toLowerCase() || ''
    return ['mp4', 'webm', 'ogv', 'mov', 'mkv', 'avi', 'mpg', 'mpeg', 'wmv', 'flv'].includes(ext)
}

/**
 * Check if file is audio
 */
export const isAudio = (path: string): boolean => {
    const ext = path.split('.').pop()?.toLowerCase() || ''
    return ['mp3', 'wav', 'ogg', 'm4a', 'aac', 'flac'].includes(ext)
}

/**
 * Check if file is audio or video
 */
export const isMediaFile = (path: string): boolean => {
    return isAudio(path) || isVideo(path)
}

/**
 * Get thumbnail URL for an entry
 */
export const getThumbnailUrl = (entry: DirectoryEntry, size: 'small' | 'large' | 'mini'): string | null => {
    if (!entry.hash || entry.hash.length === 0) return null
    const hex = Array.from(entry.hash).map(b => b.toString(16).padStart(2, '0')).join('')
    return `/api/storage/thumbnail/${hex}/${size}`
}

/**
 * Sort entries with directories first
 */
export const sortEntries = (entries: DirectoryEntry[]): DirectoryEntry[] => {
    return [...entries].sort((a, b) => {
        if (a.entry_type === 'Directory' && b.entry_type !== 'Directory') return -1
        if (a.entry_type !== 'Directory' && b.entry_type === 'Directory') return 1
        return a.path.localeCompare(b.path)
    })
}
