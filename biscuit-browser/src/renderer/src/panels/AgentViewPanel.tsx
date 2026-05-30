import { useState } from 'react'
import type { AgentView } from '@shared/types'

export function AgentViewPanel(): JSX.Element {
  const [view, setView] = useState<AgentView | null>(null)
  const [shot, setShot] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')

  const capture = async (refresh: boolean): Promise<void> => {
    setBusy(true)
    setError('')
    setShot(null)
    try {
      const v = refresh ? await window.biscuit.agentView.refresh() : await window.biscuit.agentView.get()
      setView(v)
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setBusy(false)
    }
  }

  const screenshot = async (): Promise<void> => {
    setBusy(true)
    setError('')
    try {
      const res = await window.biscuit.action.run({ kind: 'screenshot' })
      if (res.ok && typeof res.data === 'string') setShot(res.data)
      else setError(res.detail)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="panel-scroll">
      <div className="row">
        <button disabled={busy} onClick={() => void capture(false)}>
          Capture
        </button>
        <button disabled={busy} onClick={() => void capture(true)}>
          Refresh (expire refs)
        </button>
        <button disabled={busy} onClick={() => void screenshot()} title="Fallback only">
          Screenshot
        </button>
      </div>
      {error && <p style={{ color: 'var(--danger)' }}>{error}</p>}

      {view && (
        <>
          <div className="kv">
            <span className="k">URL</span>
            <span className="mono">{view.url}</span>
            <span className="k">Title</span>
            <span>{view.title}</span>
            <span className="k">Generation</span>
            <span>
              {view.generation} · {view.elements.length} elements{view.truncated ? ' · truncated' : ''}
            </span>
            {view.context && (
              <>
                <span className="k">Coverage</span>
                <span>
                  {view.context.frames.sameOrigin} same-origin / {view.context.frames.crossOrigin} cross-origin
                  frame(s) · {view.context.shadowRoots} shadow root(s)
                </span>
              </>
            )}
          </div>

          {view.context && view.context.notes.length > 0 && (
            <p className="muted">⚠ {view.context.notes.join(' · ')}</p>
          )}

          {view.headings.length > 0 && (
            <>
              <div className="section-title">Headings</div>
              <div className="scrollbox">
                {view.headings.map((h, i) => (
                  <div key={i} className="mono">
                    {'#'.repeat(Math.min(h.level, 6))} {h.text}
                  </div>
                ))}
              </div>
            </>
          )}

          <div className="section-title">Interactive elements</div>
          <div className="scrollbox">
            {view.elements.map((el) => (
              <div key={el.ref} className="elem">
                <span className="ref">{el.ref}</span> {el.role}
                {el.name ? ` "${el.name}"` : ''}
                {el.type ? ` [${el.type}]` : ''}
                {el.sensitive ? <span className="sensitive"> SENSITIVE</span> : ''}
                {el.via ? <span className="muted"> in:{el.via}</span> : ''}
                {!el.state.inViewport ? <span className="muted"> offscreen</span> : ''}
                {el.state.covered ? <span className="muted"> covered</span> : ''}
                {!el.state.enabled ? <span className="muted"> disabled</span> : ''}
              </div>
            ))}
          </div>

          <div className="section-title">Visible text</div>
          <div className="scrollbox mono" style={{ whiteSpace: 'pre-wrap' }}>
            {view.text}
          </div>
        </>
      )}

      {shot && (
        <>
          <div className="section-title">Screenshot (fallback)</div>
          <img className="screenshot" src={shot} alt="page screenshot" />
        </>
      )}

      {!view && !shot && !error && (
        <p className="muted">Capture the Agent View to see the page as the agent sees it — text and @e refs, no raw HTML.</p>
      )}
    </div>
  )
}
