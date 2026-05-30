import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import type { SettingsUpdate } from '@shared/types'

// Mutable mock state, hoisted so the electron mock factory can read it.
const h = vi.hoisted(() => ({ dir: '', enc: true }))

vi.mock('electron', () => ({
  app: { getPath: () => h.dir },
  safeStorage: {
    isEncryptionAvailable: () => h.enc,
    encryptString: (s: string) => Buffer.from('ENC(' + s + ')', 'utf8'),
    // Mirror real safeStorage: throws when the ciphertext isn't ours to decrypt
    // (e.g. keychain rotated, file moved to another machine).
    decryptString: (b: Buffer) => {
      const s = b.toString('utf8')
      if (!s.startsWith('ENC(')) throw new Error('cannot decrypt')
      return s.replace(/^ENC\(|\)$/g, '')
    }
  }
}))

import { SettingsStore } from '../src/main/settings/store'

const base: SettingsUpdate = {
  provider: 'openai',
  model: 'gpt-4o-mini',
  baseUrl: 'https://api.openai.com/v1',
  defaultMode: 'assisted',
  expertMode: false
}

const filePath = (): string => join(h.dir, 'biscuit-settings.json')

beforeEach(() => {
  h.dir = mkdtempSync(join(tmpdir(), 'biscuit-settings-'))
  h.enc = true
})

describe('SettingsStore', () => {
  it('has safe defaults (assisted, no key, expert off)', () => {
    const s = new SettingsStore().get()
    expect(s.defaultMode).toBe('assisted')
    expect(s.hasApiKey).toBe(false)
    expect(s.expertMode).toBe(false)
    expect(s.keyStorage).toBe('none')
  })

  it('encrypts the key at rest and never writes plaintext', () => {
    const store = new SettingsStore()
    store.save({ ...base, apiKey: 'sk-secret' })
    expect(store.get().keyStorage).toBe('encrypted')
    expect(store.getApiKey()).toBe('sk-secret')
    const onDisk = readFileSync(filePath(), 'utf8')
    expect(onDisk).not.toContain('sk-secret')
    expect(onDisk).toContain('apiKeyEnc')
  })

  it('persists an encrypted key across instances', () => {
    new SettingsStore().save({ ...base, apiKey: 'sk-keep' })
    const reloaded = new SettingsStore()
    expect(reloaded.getApiKey()).toBe('sk-keep')
    expect(reloaded.get().hasApiKey).toBe(true)
  })

  it('keeps the key session-only (never on disk) when the keychain is unavailable', () => {
    h.enc = false
    const store = new SettingsStore()
    store.save({ ...base, apiKey: 'sk-session' })
    expect(store.get().keyStorage).toBe('session')
    expect(store.get().secureStorageAvailable).toBe(false)
    expect(store.getApiKey()).toBe('sk-session')
    expect(readFileSync(filePath(), 'utf8')).not.toContain('sk-session')
    // A fresh instance has no key — it was never persisted.
    const reloaded = new SettingsStore()
    expect(reloaded.getApiKey()).toBeNull()
    expect(reloaded.get().hasApiKey).toBe(false)
  })

  const writeLegacy = (key: string): void => {
    const legacy = {
      provider: 'openai',
      model: 'm',
      baseUrl: 'b',
      defaultMode: 'assisted',
      expertMode: false,
      apiKeyEnc: Buffer.from('plain:' + key, 'utf8').toString('base64')
    }
    writeFileSync(filePath(), JSON.stringify(legacy), 'utf8')
  }

  it('migrates a legacy plaintext key off disk and re-encrypts it (keychain available)', () => {
    writeLegacy('sk-legacy')
    const store = new SettingsStore()
    expect(store.getApiKey()).toBe('sk-legacy')
    expect(store.get().keyStorage).toBe('encrypted')
    const onDisk = readFileSync(filePath(), 'utf8')
    expect(onDisk).not.toContain('sk-legacy')
    expect(onDisk).not.toContain('plain:')
    // Re-encrypted at rest → survives a restart.
    expect(new SettingsStore().getApiKey()).toBe('sk-legacy')
  })

  it('migrates a legacy plaintext key to session-only when the keychain is unavailable', () => {
    h.enc = false
    writeLegacy('sk-legacy')
    const store = new SettingsStore()
    expect(store.getApiKey()).toBe('sk-legacy')
    expect(store.get().keyStorage).toBe('session')
    const onDisk = readFileSync(filePath(), 'utf8')
    expect(onDisk).not.toContain('sk-legacy')
    expect(onDisk).not.toContain('plain:')
    // Not persisted → gone after restart.
    expect(new SettingsStore().getApiKey()).toBeNull()
  })

  it('treats an un-decryptable stored key as no key (re-prompt, not a silent 401)', () => {
    const corrupt = {
      provider: 'openai',
      model: 'm',
      baseUrl: 'b',
      defaultMode: 'assisted',
      expertMode: false,
      apiKeyEnc: Buffer.from('not-our-ciphertext', 'utf8').toString('base64')
    }
    writeFileSync(filePath(), JSON.stringify(corrupt), 'utf8')
    const store = new SettingsStore()
    expect(store.getApiKey()).toBeNull()
    expect(store.get().hasApiKey).toBe(false)
    expect(store.get().keyStorage).toBe('none')
  })

  it('coerces a Bypass default to Assisted (never persists Bypass)', () => {
    const saved = new SettingsStore().save({ ...base, defaultMode: 'bypass' })
    expect(saved.defaultMode).toBe('assisted')
  })

  it('tracks expert mode', () => {
    const store = new SettingsStore()
    expect(store.isExpertMode()).toBe(false)
    store.save({ ...base, expertMode: true })
    expect(store.isExpertMode()).toBe(true)
    expect(store.get().expertMode).toBe(true)
  })

  it('clears the key when an empty string is saved', () => {
    const store = new SettingsStore()
    store.save({ ...base, apiKey: 'sk-x' })
    store.save({ ...base, apiKey: '' })
    expect(store.getApiKey()).toBeNull()
    expect(store.get().hasApiKey).toBe(false)
  })
})
