// Typed API surface generated from the server's OpenAPI spec.
//
// `schema.d.ts` is produced by `openapi-typescript` from `openapi.json` (run
// `npm run generate`, refreshed from Rust via `make openapi-spec`). Everything
// downstream — services, components — imports its request/response types from
// here, so the frontend can never silently drift from the server contract.
// Do not hand-write model interfaces; add the missing field to the Rust DTO
// and regenerate instead.

import type { components } from './schema'

type Schemas = components['schemas']

// --- Response models ---
export type User = Schemas['UserResponse']
export type Group = Schemas['GroupResponse']
export type Tag = Schemas['TagResponse']
export type Storage = Schemas['StorageResponse']
export type Shared = Schemas['ShareResponse']
export type Photo = Schemas['PhotoResponse']
export type MetaResponse = Schemas['MetaResponse']
export type ShareInfoResponse = Schemas['ShareInfoResponse']
export type EntryType = Schemas['EntryType']

// --- Request bodies ---
export type CreateUserRequest = Schemas['CreateUserRequest']
export type UpdateUserRequest = Schemas['UpdateUserRequest']
export type ChangePasswordRequest = Schemas['ChangePasswordRequest']
export type CreateGroupRequest = Schemas['CreateGroupRequest']
export type UpdateGroupRequest = Schemas['UpdateGroupRequest']
export type CreateTagRequest = Schemas['CreateTagRequest']
export type UpdateTagRequest = Schemas['UpdateTagRequest']
export type CreateStorageRequest = Schemas['CreateStorageRequest']
export type UpdateStorageRequest = Schemas['UpdateStorageRequest']
export type ShareRequest = Schemas['ShareEntryRequest']
export type CreateEntryRequest = Schemas['CreateEntryRequest']
export type RenameEntryRequest = Schemas['RenameEntryRequest']
export type UpdateEntryTagsRequest = Schemas['UpdateEntryTagsRequest']

// The API's `DirectoryEntry` DTO does not carry per-entry `tags` — they live in
// the `meta` row keyed by the entry hash. The file browser still reads
// `entry.tags`, so it stays a client-side optional field here rather than being
// invented back onto the server type. Tracked as drift bug B7.
export type DirectoryEntry = Schemas['DirectoryEntry'] & { tags?: number[] }

export type DirectoryListingResponse = Omit<
  Schemas['DirectoryListingResponse'],
  'entries'
> & { entries: DirectoryEntry[] }

/** A single page of a paginated list endpoint (`Page<T>` on the server). */
export interface Page<T> {
  items: T[]
  page: number
  per_page: number
  total: number
  total_pages: number
}

export type { paths, components, operations } from './schema'
