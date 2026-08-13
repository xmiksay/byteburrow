import { createRouter, createWebHistory } from 'vue-router'
import type { RouteRecordRaw } from 'vue-router'
import { useAuth } from '../composables/useAuth'

// Lazy load components for better performance
const FileExplorer = () => import('../components/FileExplorer.vue')
const SharedWithMe = () => import('../components/SharedWithMe.vue')
const StorageManagement = () => import('../components/StorageManagement.vue')
const UserManagement = () => import('../components/UserManagement.vue')
const GroupManagement = () => import('../components/GroupManagement.vue')
const TagManagement = () => import('../components/TagManagement.vue')
const ShareManagement = () => import('../components/ShareManagement.vue')
const Monitoring = () => import('../components/Monitoring.vue')
const PhotoLibrary = () => import('../components/PhotoLibrary.vue')

const routes: RouteRecordRaw[] = [
    {
        path: '/',
        redirect: '/files'
    },
    {
        path: '/files/:storageId?/:path(.*)?',
        name: 'files',
        component: FileExplorer,
        meta: { title: 'Files' }
    },
    {
        path: '/shared/:shareId(\\d+)/:path(.*)?',
        name: 'shared',
        component: SharedWithMe,
        meta: { title: 'Shared with Me' }
    },
    {
        path: '/shared',
        name: 'shared-root',
        component: SharedWithMe,
        meta: { title: 'Shared with Me' }
    },
    {
        path: '/photos/:year?/:month?/:day?',
        name: 'photos',
        component: PhotoLibrary,
        meta: { title: 'Photos' }
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
        path: '/shared/:token',
        name: 'public-share',
        component: () => import('../components/PublicShare.vue'),
        meta: { title: 'Shared Content', public: true }
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

// Auth guard: protect every route that isn't marked public. When the user
// is unauthenticated, send them to the default `/files` route — App.vue
// renders the login overlay there — and carry a `redirect` query so we can
// bounce back to the requested page after a successful login.
router.beforeEach((to) => {
    if (to.meta.public) {
        return true
    }

    const { isAuthenticated } = useAuth()
    if (!isAuthenticated.value) {
        // Avoid an infinite redirect loop when already heading to the default route.
        if (to.path === '/files') {
            return true
        }
        return { path: '/files', query: { redirect: to.fullPath } }
    }

    return true
})

// Update page title on route change
router.afterEach((to) => {
    document.title = to.meta.title ? `${to.meta.title} - ByteBurrow` : 'ByteBurrow'
})

export default router
