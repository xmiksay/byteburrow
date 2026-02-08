import { createRouter, createWebHistory } from 'vue-router'
import type { RouteRecordRaw } from 'vue-router'

// Lazy load components for better performance
const FileExplorer = () => import('../components/FileExplorer.vue')
const SharedWithMe = () => import('../components/SharedWithMe.vue')
const StorageManagement = () => import('../components/StorageManagement.vue')
const UserManagement = () => import('../components/UserManagement.vue')
const GroupManagement = () => import('../components/GroupManagement.vue')
const TagManagement = () => import('../components/TagManagement.vue')
const ShareManagement = () => import('../components/ShareManagement.vue')
const Monitoring = () => import('../components/Monitoring.vue')

const routes: RouteRecordRaw[] = [
    {
        path: '/',
        redirect: '/files'
    },
    {
        path: '/files',
        name: 'files',
        component: FileExplorer,
        meta: { title: 'Files' }
    },
    {
        path: '/shared',
        name: 'shared',
        component: SharedWithMe,
        meta: { title: 'Shared with Me' }
    },
    {
        path: '/storages',
        name: 'storages',
        component: StorageManagement,
        meta: { title: 'Storage Management' }
    },
    {
        path: '/users',
        name: 'users',
        component: UserManagement,
        meta: { title: 'User Management' }
    },
    {
        path: '/groups',
        name: 'groups',
        component: GroupManagement,
        meta: { title: 'Group Management' }
    },
    {
        path: '/tags',
        name: 'tags',
        component: TagManagement,
        meta: { title: 'Tag Management' }
    },
    {
        path: '/shares',
        name: 'shares',
        component: ShareManagement,
        meta: { title: 'Share Management' }
    },
    {
        path: '/monitoring',
        name: 'monitoring',
        component: Monitoring,
        meta: { title: 'Monitoring' }
    },
    {
        // Catch-all route - redirect to files
        path: '/:pathMatch(.*)*',
        redirect: '/files'
    }
]

const router = createRouter({
    history: createWebHistory(),
    routes
})

// Update page title on route change
router.afterEach((to) => {
    document.title = to.meta.title ? `${to.meta.title} - Cloud` : 'Cloud'
})

export default router
