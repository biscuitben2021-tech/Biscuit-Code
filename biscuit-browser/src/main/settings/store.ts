import { app, safeStorage } from 'electron'
import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import type { LlmProvider, Settings, SettingsUpdate, PermissionMode } from '@shared/types'

interface PersistedSettings {
  provider: LlmProvider
  model: string
  baseUrl: string
  defaultMode: PermissionMode
  /** Base64 of an encrypted API key (via Electron safeStorage), or empty. */
  apiKeyEnc: string
}

const DEFAULTS: PersistedSettings = {
  provider: 'openai',
  model: 'gpt-4o-mini',
  baseUrl: 'https://api.openai.com/v1',
  defaultMode: 'assisted',
  apiKeyEnc: ''
}

/**
 * Local settings store. The API key is encrypted at rest with the OS keychain
 * (Electron safeStorage) and lives only in the main process — it is never sent
 * to the renderer or to any browsed page.
 */
export class SettingsStore {
  private readonly file: string
  private data: PersistedSettings

  constructor() {
    this.file = join(app.getPath('userData'), 'biscuit-settings.json')
    this.data = this.load()
  }

  private load(): PersistedSettings {
    try {
      if (existsSync(this.file)) {
        const raw = JSON.parse(readFileSync(this.file, 'utf8')) as Partial<PersistedSettings>
        return { ...DEFAULTS, ...raw }
      }
    } catch (err) {
      console.error('[settings] failed to load, using defaults:', err)
    }
    return { ...DEFAULTS }
  }

  private persist(): void {
    try {
      writeFileSync(this.file, JSON.stringify(this.data, null, 2), 'utf8')
    } catch (err) {
      console.error('[settings] failed to persist:', err)
    }
  }

  /** Renderer-facing view: never includes the raw key. */
  get(): Settings {
    return {
      provider: this.data.provider,
      model: this.data.model,
      baseUrl: this.data.baseUrl,
      defaultMode: this.data.defaultMode,
      hasApiKey: this.data.apiKeyEnc.length > 0
    }
  }

  save(update: SettingsUpdate): Settings {
    this.data.provider = update.provider
    this.data.model = update.model
    this.data.baseUrl = update.baseUrl
    this.data.defaultMode = update.defaultMode
    if (update.apiKey !== undefined) {
      const key = update.apiKey.trim()
      if (key.length === 0) {
        this.data.apiKeyEnc = '' // explicit clear
      } else if (safeStorage.isEncryptionAvailable()) {
        this.data.apiKeyEnc = safeStorage.encryptString(key).toString('base64')
      } else {
        // Fallback: store plain (still local-only). Warn the user in the UI.
        this.data.apiKeyEnc = Buffer.from(`plain:${key}`).toString('base64')
      }
    }
    this.persist()
    return this.get()
  }

  /** Decrypted key for main-process LLM calls only. Returns null if unset. */
  getApiKey(): string | null {
    if (!this.data.apiKeyEnc) return null
    try {
      const buf = Buffer.from(this.data.apiKeyEnc, 'base64')
      const asPlain = buf.toString('utf8')
      if (asPlain.startsWith('plain:')) return asPlain.slice('plain:'.length)
      return safeStorage.decryptString(buf)
    } catch (err) {
      console.error('[settings] failed to decrypt api key:', err)
      return null
    }
  }
}
