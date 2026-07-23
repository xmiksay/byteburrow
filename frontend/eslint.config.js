import pluginVue from 'eslint-plugin-vue'
import { defineConfigWithVueTs, vueTsConfigs } from '@vue/eslint-config-typescript'

export default defineConfigWithVueTs(
  {
    name: 'app/files-to-lint',
    files: ['**/*.{ts,mts,tsx,vue}'],
  },
  {
    name: 'app/files-to-ignore',
    // src/api/schema.d.ts is generated from the OpenAPI spec — never lint it.
    ignores: ['dist/**', 'node_modules/**', 'coverage/**', 'src/api/schema.d.ts'],
  },
  pluginVue.configs['flat/essential'],
  vueTsConfigs.recommended,
  {
    rules: {
      // These surface real cleanup targets in the existing code, but are demoted to
      // warnings so adopting ESLint doesn't block CI on a pre-existing backlog.
      // `any` at the API boundary (utils/api.ts, services/*), prop mutation in the
      // viewers, and single-word view names (Login, Monitoring) are all pre-existing.
      '@typescript-eslint/no-explicit-any': 'warn',
      'vue/no-mutating-props': 'warn',
      'vue/multi-word-component-names': 'off',
    },
  },
)
