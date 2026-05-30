import { app, safeStorage } from 'electron'
import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import type { LlmProvider, Settings, SettingsUpdate, PermissionMode } from '@shared/types'

interface PersistedSettings {
  provider: LlmProvider
  model: string
  baseUrl: string
  defaultMode: PermissionMode
  expertMode: boolean
  /** Base64 of an OS-encrypted API key (Electron safeStorage), or empty. */
  apiKeyEnc: string
}

const DEFAULTS: PersistedSettings = {
  provider: 'openai',
  model: 'gpt-4o-mini',
  baseUrl: 'https://api.openai.com/v1',
  defaultMode: 'assisted',
  expertMode: false,
  apiKeyEnc: ''
}

// Legacy marker: older builds base64-encoded `plain:<key>` to disk when the OS
// keychain was unavailable. We no longer write plaintext; on load we migrate any
// such value into a session-only key and scrub it from disk.
const LEGACY_PLAIN = 'plain:'

/**
 * Bypass is never a valid saved default — it must be re-armed each session via
 * an explicit typed confirmation. Coerce any attempt to persist it.
 */
function safeDefaultMode(mode: PermissionMode): PermissionMode {
  return mode === 'bypass' ? 'assisted' : mode
}

/**
 * Local settings store. When the OS keychain (Electron safeStorage) is
 * available, the API key is encrypted at rest. When it is NOT available we never
 * write the key to disk in plaintext — instead it is held in memory for the
 * current session only and the UI warns the user. The key lives only in the main
 * process and is never sent to the renderer or to any browsed page.
 */
export class SettingsStore {
  private readonly file: string
  private data: PersistedSettings
  /** In-memory, never-persisted key used when the keychain is unavailable. */
  private sessionKey: string | null = null

  constructor() {
    this.file = join(app.getPath('userData'), 'biscuit-settings.json')
    this.data = this.load()
  }

  private load(): PersistedSettings {
    try {
      if (existsSync(this.file)) {
        const raw = JSON.parse(readFileSync(this.file, 'utf8')) as Partial<PersistedSettings>
        const merged = { ...DEFAULTS, ...raw }
        // Never boot into Bypass, even if an old/edited file asks for it.
        merged.defaultMode = safeDefaultMode(merged.defaultMode)
        // Migrate any legacy plaintext key off disk. If the OS keychain is now
        // available, re-encrypt it so it survives restarts; otherwise fall back
        // to a session-only key. Either way the plaintext is scrubbed from disk.
        if (merged.apiKeyEnc) {
          try {
            const decoded = Buffer.from(merged.apiKeyEnc, 'base64').toString('utf8')
            if (decoded.startsWith(LEGACY_PLAIN)) {
              const recovered = decoded.slice(LEGACY_PLAIN.length)
              if (this.secureStorageAvailable()) {
                merged.apiKeyEnc = safeStorage.encryptString(recovered).toString('base64')
              } else {
                this.sessionKey = recovered
                merged.apiKeyEnc = ''
              }
              this.data = merged
              this.persist() // remove the plaintext from disk immediately
            }
          } catch {
            /* not a legacy plaintext value — leave as-is */
          }
        }
        return merged
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

  /** True only if a stored ciphertext can actually be decrypted right now. */
  private encryptedKeyUsable(): boolean {
    if (!this.data.apiKeyEnc) return false
    try {
      return safeStorage.decryptString(Buffer.from(this.data.apiKeyEnc, 'base64')).length > 0
    } catch {
      return false
    }
  }

  private keyStorage(): Settings['keyStorage'] {
    if (this.encryptedKeyUsable()) return 'encrypted'
    if (this.sessionKey) return 'session'
    return 'none'
  }

  private secureStorageAvailable(): boolean {
    try {
      return safeStorage.isEncryptionAvailable()
    } catch {
      return false
    }
  }

  /** Renderer-facing view: never includes the raw key. */
  get(): Settings {
    return {
      provider: this.data.provider,
      model: this.data.model,
      baseUrl: this.data.baseUrl,
      defaultMode: this.data.defaultMode,
      expertMode: this.data.expertMode,
      // Reflect a *usable* key, not just the presence of ciphertext bytes — an
      // un-decryptable blob (keychain rotated, moved machine) reads as no key so
      // the UI prompts the user to re-enter rather than silently 401ing.
      hasApiKey: this.encryptedKeyUsable() || this.sessionKey !== null,
      keyStorage: this.keyStorage(),
      secureStorageAvailable: this.secureStorageAvailable()
    }
  }

  /** Whether Bypass mode may be armed this session. */
  isExpertMode(): boolean {
    return this.data.expertMode
  }

  save(update: SettingsUpdate): Settings {
    this.data.provider = update.provider
    this.data.model = update.model
    this.data.baseUrl = update.baseUrl
    this.data.defaultMode = safeDefaultMode(update.defaultMode)
    this.data.expertMode = update.expertMode === true
    if (update.apiKey !== undefined) {
      const key = update.apiKey.trim()
      if (key.length === 0) {
        // Explicit clear.
        this.data.apiKeyEnc = ''
        this.sessionKey = null
      } else if (this.secureStorageAvailable()) {
        this.data.apiKeyEnc = safeStorage.encryptString(key).toString('base64')
        this.sessionKey = null
      } else {
        // No OS keychain: keep the key in memory only — NEVER write plaintext to
        // disk. The UI surfaces keyStorage === 'session' so the user knows it
        // won't persist across restarts.
        this.sessionKey = key
        this.data.apiKeyEnc = ''
      }
    }
    this.persist()
    return this.get()
  }

  /** Decrypted key for main-process LLM calls only. Returns null if unset. */
  getApiKey(): string | null {
    if (this.sessionKey) return this.sessionKey
    if (!this.data.apiKeyEnc) return null
    try {
      return safeStorage.decryptString(Buffer.from(this.data.apiKeyEnc, 'base64'))
    } catch (err) {
      console.error('[settings] failed to decrypt api key:', err)
      return null
    }
  }
}
