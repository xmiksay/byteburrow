/** A single page of results returned by a paginated list endpoint. */
export interface Page<T> {
    items: T[]
    page: number
    per_page: number
    total: number
    total_pages: number
}

export interface User {
    id: number
    name: string
    description?: string
    username: string
    enabled: boolean
    admin: boolean
}

export interface Group {
    id: number
    name: string
    description?: string
}

export interface Tag {
    id: number
    user_id: number
    name: string
    description?: string
}

export interface CreateTagRequest {
    name: string;
    description?: string;
}

export interface UpdateTagRequest {
    name?: string;
    description?: string;
}

export interface Storage {
    id: number
    name: string
    description?: string
    path: string
    default_user: number
    default_group: number
}

export interface CreateStorageRequest {
    name: string
    description?: string
    path: string
    default_user: number
    default_group: number
}

export interface UpdateStorageRequest {
    name?: string
    description?: string
    path?: string
    default_user?: number
    default_group?: number
}

export interface CreateGroupRequest {
    name: string
    description?: string
}

export interface UpdateGroupRequest {
    name?: string
    description?: string
}

export interface DirectoryEntry {
    id?: number
    storage_id: number
    user_id: number
    group_id: number
    parent_id?: number
    path: string
    hash?: number[]
    entry_type: 'File' | 'Directory' | 'Symlink'
    size?: number
    tags: number[]
    modified_at?: string
    created_at: string
}

export interface DirectoryListingResponse {
    storage_id: number
    path: string
    entries: DirectoryEntry[]
}

export interface Shared {
    id: number
    storage_id: number
    path: string
    path_id: number
    token?: string
    has_public_link: boolean
    can_write: boolean
    user_ids: number[]
    group_ids: number[]
    expires_at?: string
    created_at: string
}

export interface Photo {
    hash: string
    storage_id?: number
    path?: string
    latitude?: number
    longitude?: number
    date?: string
    keywords: string[]
}

export interface MetaResponse {
    hash: string
    tags: number[]
    keywords: string[]
    custom: Record<string, any>
}

export interface ShareRequest {
    can_write: boolean
    expires_in_days?: number
    user_ids?: number[]
    group_ids?: number[]
    public_link: boolean
}
