import { useEffect, useState } from 'react'
import { PERMISSION_MODES, describeMode, type LlmProvider, type PermissionMode } from '@shared/types'
import { patchState, useBiscuit } from '../state/store'

const PRESETS: Record<LlmProvider, { label: string; baseUrl: string; model: string }> = {
  openai: { label: 'OpenAI', baseUrl: 'https://api.openai.com/v1', model: 'gpt-4o-mini' },
  anthropic: { label: 'Anthropic', baseUrl: 'https://api.anthropic.com/v1', model: 'claude-sonnet-4-20250514' },
  google: { label: 'Google Gemini', baseUrl: 'https://generativelanguage.googleapis.com/v1beta', model: 'gemini-2.0-flash' },
  openai_compatible: { label: 'OpenAI-compatible', baseUrl: 'http://localhost:8000/v1', model: '' },
  lmstudio: { label: 'LM Studio', baseUrl: 'http://localhost:1234/v1', model: '' }
}

export function SettingsPanel(): JSX.Element {
  const { settings } = useBiscuit()
  const [provider, setProvider] = useState<LlmProvider>('openai')
  const [model, setModel] = useState('')
  const [baseUrl, setBaseUrl] = useState('')
  const [defaultMode, setDefaultMode] = useState<PermissionMode>('assisted')
  const [apiKey, setApiKey] = useState('')
  const [saved, setSaved] = useState('')

  useEffect(() => {
    if (!settings) return
    setProvider(settings.provider)
    setModel(settings.model)
    setBaseUrl(settings.baseUrl)
    setDefaultMode(settings.defaultMode)
  }, [settings?.provider, settings?.model, settings?.baseUrl, settings?.defaultMode])

  const onProvider = (p: LlmProvider): void => {
    setProvider(p)
    // Seed sensible defaults; the user can still edit.
    setBaseUrl(PRESETS[p].baseUrl)
    setModel(PRESETS[p].model)
  }

  const save = async (): Promise<void> => {
    const result = await window.biscuit.settings.save({
      provider,
      model: model.trim(),
      baseUrl: baseUrl.trim(),
      defaultMode,
      apiKey: apiKey.length ? apiKey : undefined // omitted = keep existing
    })
    patchState({ settings: result })
    setApiKey('')
    setSaved('Saved.')
    window.setTimeout(() => setSaved(''), 1500)
  }

  return (
    <div className="panel-scroll">
      <div className="field">
        <label>Provider</label>
        <select value={provider} onChange={(e) => onProvider(e.target.value as LlmProvider)}>
          {Object.entries(PRESETS).map(([key, p]) => (
            <option key={key} value={key}>
              {p.label}
            </option>
          ))}
        </select>
      </div>

      <div className="field">
        <label>Model</label>
        <input value={model} placeholder="model id" onChange={(e) => setModel(e.target.value)} />
      </div>

      <div className="field">
        <label>Base URL</label>
        <input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} />
      </div>

      <div className="field">
        <label>API key {settings?.hasApiKey ? '(a key is stored)' : '(none stored)'}</label>
        <input
          type="password"
          value={apiKey}
          placeholder={settings?.hasApiKey ? '•••••• (leave blank to keep)' : 'paste API key'}
          onChange={(e) => setApiKey(e.target.value)}
        />
        <span className="muted">
          Stored locally and encrypted with the OS keychain. Never sent to web pages; used only by the
          main process for model calls.
        </span>
      </div>

      <div className="field">
        <label>Default permission mode</label>
        <select value={defaultMode} onChange={(e) => setDefaultMode(e.target.value as PermissionMode)}>
          {PERMISSION_MODES.map((m) => (
            <option key={m} value={m}>
              {m} — {describeMode(m)}
            </option>
          ))}
        </select>
      </div>

      <div className="row">
        <button onClick={save}>Save settings</button>
        {saved && <span style={{ color: 'var(--ok)' }}>{saved}</span>}
      </div>

      <p className="muted">
        TODO(phase-later): connect to the Rust “biscuits” CLI as an alternative model backend (shared
        provider config). See README.
      </p>
    </div>
  )
}
