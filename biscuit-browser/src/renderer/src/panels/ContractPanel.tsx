import { useEffect, useState } from 'react'
import type { TaskContract } from '@shared/types'
import { useBiscuit } from '../state/store'

function Pills({ items, kind }: { items: string[]; kind: 'allow' | 'confirm' | 'block' }): JSX.Element {
  if (!items.length) return <span className="muted">none</span>
  return (
    <span>
      {items.map((a) => (
        <span key={a} className={`pill ${kind}`}>
          {a}
        </span>
      ))}
    </span>
  )
}

export function ContractPanel(): JSX.Element {
  const { contract, mode } = useBiscuit()
  const [draft, setDraft] = useState('')
  const [error, setError] = useState('')
  const [preview, setPreview] = useState('')

  // Sync the editable JSON whenever the locked/draft contract changes.
  useEffect(() => {
    setDraft(contract.contract ? JSON.stringify(contract.contract, null, 2) : '')
    setError('')
  }, [JSON.stringify(contract.contract), contract.status])

  const lock = (): void => {
    try {
      const parsed = JSON.parse(draft) as TaskContract
      void window.biscuit.contract.lock(parsed)
      setError('')
    } catch (e) {
      setError(`Invalid JSON: ${(e as Error).message}`)
    }
  }

  const c = contract.contract

  return (
    <div className="panel-scroll">
      <div className="kv">
        <span className="k">Status</span>
        <span>
          <b>{contract.status}</b>
          {contract.status === 'locked' && <span className="muted"> (enforced by the Action Gate)</span>}
        </span>
        <span className="k">Mode</span>
        <span>{mode}</span>
      </div>

      {contract.status === 'none' && (
        <p className="muted">
          No active contract. Send a message in Chat to generate one from your request. The contract is
          derived from your prompt only — never from page content.
        </p>
      )}

      {c && (
        <>
          <div className="kv">
            <span className="k">Goal</span>
            <span>{c.goal}</span>
            <span className="k">Allowed</span>
            <Pills items={c.allowed_actions} kind="allow" />
            <span className="k">Confirm first</span>
            <Pills items={c.requires_user_confirmation} kind="confirm" />
            <span className="k">Blocked</span>
            <Pills items={c.blocked_without_user_override} kind="block" />
          </div>

          <div className="section-title">Edit & lock</div>
          {mode === 'safe' && contract.status === 'draft' && (
            <p className="muted">Safe mode: review/edit, then lock to start the task.</p>
          )}
          <textarea rows={12} value={draft} onChange={(e) => setDraft(e.target.value)} />
          {error && <p style={{ color: 'var(--danger)' }}>{error}</p>}
          <div className="row">
            <button onClick={lock}>{contract.status === 'draft' ? 'Lock & Start' : 'Re-lock'}</button>
            <button onClick={() => void window.biscuit.contract.clear()}>Clear</button>
          </div>
        </>
      )}

      <div className="section-title">Preview a contract</div>
      <p className="muted">Generate a contract from an arbitrary prompt without starting a task.</p>
      <textarea
        rows={2}
        placeholder="e.g. Find the latest stable Rust version from the official docs"
        value={preview}
        onChange={(e) => setPreview(e.target.value)}
      />
      <div className="row">
        <button
          disabled={!preview.trim()}
          onClick={async () => {
            const generated = await window.biscuit.contract.generate(preview.trim())
            setDraft(JSON.stringify(generated, null, 2))
          }}
        >
          Generate preview
        </button>
      </div>
    </div>
  )
}
