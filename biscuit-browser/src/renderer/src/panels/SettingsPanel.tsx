import { useEffect, useState } from 'react'
import {
  PERMISSION_MODES,
  describeMode,
  type LlmProvider,
  type McpInfo,
  type PermissionMode
} from '@shared/types'
import { patchState, useBiscuit } from '../state/store'

const PRESETS: Record<LlmProvider, { label: string; baseUrl: string; model: string }> = {
  openai: { label: 'OpenAI', baseUrl: 'https://api.openai.com/v1', model: 'gpt-4o-mini' },
  anthropic: {
    label: 'Anthropic',
    baseUrl: 'https://api.anthropic.com/v1',
    model: 'claude-sonnet-4-20250514'
  },
  google: {
    label: 'Google Gemini',
    baseUrl: 'https://generativelanguage.googleapis.com/v1beta',
    model: 'gemini-2.0-flash'
  },
  openai_compatible: { label: 'OpenAI-compatible', baseUrl: 'http://localhost:8000/v1', model: '' },
  lmstudio: { label: 'LM Studio', baseUrl: 'http://localhost:1234/v1', model: '' }
}

export function SettingsPanel(): JSX.Element {
  const { settings } = useBiscuit()
  const [provider, setProvider] = useState<LlmProvider>('openai')
  const [model, setModel] = useState('')
  const [baseUrl, setBaseUrl] = useState('')
  const [defaultMode, setDefaultMode] = useState<PermissionMode>('assisted')
  const [expertMode, setExpertMode] = useState(false)
  const [apiKey, setApiKey] = useState('')
  const [saved, setSaved] = useState('')
  const [mcp, setMcp] = useState<McpInfo | null>(null)
  const [copied, setCopied] = useState(false)

  useEffect(() => {
    void window.biscuit.mcp.getInfo().then(setMcp)
  }, [])

  useEffect(() => {
    if (!settings) return
    setProvider(settings.provider)
    setModel(settings.model)
    setBaseUrl(settings.baseUrl)
    setDefaultMode(settings.defaultMode)
    setExpertMode(settings.expertMode)
  }, [settings?.provider, settings?.model, settings?.baseUrl, settings?.defaultMode, settings?.expertMode])

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
      expertMode,
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
        {settings && !settings.secureStorageAvailable ? (
          <span className="warn-text">
            ⚠ Your OS keychain is unavailable, so the key will <b>not</b> be written to disk. It is kept in
            memory for this session only and you'll need to re-enter it after restarting. Biscuit never stores
            an API key in plaintext.
          </span>
        ) : (
          <span className="muted">
            Stored locally and encrypted with the OS keychain. Never sent to web pages; used only by the main
            process for model calls.
          </span>
        )}
        {settings?.keyStorage === 'session' && (
          <span className="muted">Current key storage: session-only (in memory).</span>
        )}
      </div>

      <div className="field">
        <label>Default permission mode</label>
        <select value={defaultMode} onChange={(e) => setDefaultMode(e.target.value as PermissionMode)}>
          {/* Bypass can never be a saved default — it must be armed per session. */}
          {PERMISSION_MODES.filter((m) => m !== 'bypass').map((m) => (
            <option key={m} value={m}>
              {m} — {describeMode(m)}
            </option>
          ))}
        </select>
      </div>

      <div className="field">
        <label className="checkbox">
          <input type="checkbox" checked={expertMode} onChange={(e) => setExpertMode(e.target.checked)} />
          <span>Expert mode — unlock Bypass</span>
        </label>
        <span className="muted">
          Bypass lets the agent act with no approvals and no Task Contract. Enabling expert mode only unlocks
          it; arming Bypass still requires a typed confirmation each session, and the emergency Stop drops
          back to Assisted.
        </span>
      </div>

      <div className="row">
        <button onClick={save}>Save settings</button>
        {saved && <span style={{ color: 'var(--ok)' }}>{saved}</span>}
      </div>

      <div className="section-title">Agent connection (MCP)</div>
      <p className="muted">
        Biscuit Browser runs a local <b>MCP server</b> so the Biscuits CLI and other AI agents can drive it —
        reading the page as an Agent View and acting through the same Action Gate.
      </p>
      {mcp?.running ? (
        <div className="field">
          <label>MCP endpoint</label>
          <div className="row">
            <input readOnly value={mcp.url} onFocus={(e) => e.currentTarget.select()} />
            <button
              onClick={async () => {
                try {
                  await navigator.clipboard.writeText(mcp.url)
                  setCopied(true)
                  window.setTimeout(() => setCopied(false), 1500)
                } catch {
                  /* clipboard unavailable */
                }
              }}
            >
              {copied ? 'Copied' : 'Copy'}
            </button>
          </div>
          <span className="muted">
            Localhost only. In the Biscuits CLI, add it with <code>/mcp</code> (HTTP transport). Tools:
            browser_get_agent_view, browser_open_url, browser_click, browser_type, browser_scroll,
            browser_screenshot, browser_list_tabs, browser_new_tab, browser_status.
          </span>
        </div>
      ) : (
        <p className="muted">MCP server is starting…</p>
      )}

      <p className="muted">
        TODO(phase-later): use the Rust “biscuits” CLI as an alternative model backend (shared provider
        config). See README.
      </p>
    </div>
  )
}
