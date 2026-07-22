import { describe, it, expect } from 'vitest'
import {
  formatSize,
  getBasename,
  isViewableAsText,
  isImage,
  isVideo,
  isAudio,
  isMediaFile,
  getThumbnailUrl,
  sortEntries,
} from './file'
import type { DirectoryEntry } from '../types'

const entry = (over: Partial<DirectoryEntry>): DirectoryEntry =>
  ({ path: 'x', entry_type: 'File', ...over }) as DirectoryEntry

describe('formatSize', () => {
  it('returns empty string for null/undefined', () => {
    expect(formatSize(null)).toBe('')
    expect(formatSize(undefined)).toBe('')
  })

  it('keeps bytes below 1 KiB with a B suffix', () => {
    expect(formatSize(0)).toBe('0 B')
    expect(formatSize(512)).toBe('512 B')
    expect(formatSize(1023)).toBe('1023 B')
  })

  it('scales up to larger units', () => {
    expect(formatSize(1024)).toBe('1 KB')
    expect(formatSize(1024 * 1024)).toBe('1 MB')
    expect(formatSize(5 * 1024 * 1024 * 1024)).toBe('5 GB')
  })
})

describe('getBasename', () => {
  it('extracts the trailing path segment', () => {
    expect(getBasename('/a/b/c.txt')).toBe('c.txt')
    expect(getBasename('a/b/')).toBe('b')
  })

  it('falls back to the whole path when there is no segment', () => {
    expect(getBasename('')).toBe('')
    expect(getBasename('/')).toBe('/')
  })
})

describe('file-type predicates', () => {
  it('classifies text/image/video/audio by extension', () => {
    expect(isViewableAsText('main.rs')).toBe(true)
    expect(isViewableAsText('photo.png')).toBe(false)
    expect(isImage('PHOTO.JPG')).toBe(true)
    expect(isVideo('clip.mkv')).toBe(true)
    expect(isAudio('song.flac')).toBe(true)
    expect(isMediaFile('a.png')).toBe(true)
    expect(isMediaFile('a.txt')).toBe(false)
  })
})

describe('getThumbnailUrl', () => {
  it('returns null when there is no hash', () => {
    expect(getThumbnailUrl(entry({ hash: undefined }), 'small')).toBeNull()
    expect(getThumbnailUrl(entry({ hash: [] }), 'small')).toBeNull()
  })

  it('hex-encodes the hash into the thumbnail path', () => {
    expect(getThumbnailUrl(entry({ hash: [0, 15, 255] }), 'large')).toBe(
      '/api/storage/thumbnail/000fff/large',
    )
  })
})

describe('sortEntries', () => {
  it('lists directories first, then sorts by path', () => {
    const input = [
      entry({ path: 'b.txt', entry_type: 'File' }),
      entry({ path: 'z', entry_type: 'Directory' }),
      entry({ path: 'a.txt', entry_type: 'File' }),
      entry({ path: 'a', entry_type: 'Directory' }),
    ]
    expect(sortEntries(input).map(e => e.path)).toEqual(['a', 'z', 'a.txt', 'b.txt'])
  })

  it('does not mutate the input array', () => {
    const input = [entry({ path: 'b' }), entry({ path: 'a' })]
    sortEntries(input)
    expect(input.map(e => e.path)).toEqual(['b', 'a'])
  })
})
