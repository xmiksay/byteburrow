import { api } from '../utils/api'

export interface AppConfig {
    base_url: string
}

let cachedConfig: AppConfig | null = null

export const configService = {
    async getConfig(): Promise<AppConfig> {
        if (cachedConfig) return cachedConfig
        cachedConfig = await api.get<AppConfig>('/api/config')
        return cachedConfig
    }
}
