import { useEffect, useState } from 'react'
import type { TaskContract } from '@shared/types'
import { useBiscuit } from '../state/store'
import { contractToTodo } from '../contractText'

export function ContractPanel(): JSX.Element {
  const { contract, mode } = useBiscuit()
  const [draft, setDraft] = useState('')
  const [error, setError] = useState('')
  const [showJson, setShowJson] = useState(false)
  const [preview, setPreview] = useState('')
  const [previewContract, setPreviewContract] = useState<TaskContract | null>(null)

  // Keep the editable JSON in sync with the current contract.
  useEffect(() => {
    setDraft(contract.contract ? JSON.stringify(contract.contract, null, 2) : '')
    setError('')
  }, [JSON.stringify(contract.contract), contract.status])

  const lockEdited = (): void => {
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
          {contract.status === 'locked' && <span className="muted"> · enforced by the Action Gate</span>}
        </span>
        <span className="k">Mode</span>
        <span>{mode}</span>
      </div>

      {contract.status === 'none' && (
        <p className="muted">
          No active task plan. Send a message in Chat to generate one from your request. The plan is derived
          from your prompt only — never from page content.
        </p>
      )}

      {c && (
        <>
          <div className="section-title">Task plan</div>
          <pre className="todo-box">{contractToTodo(c)}</pre>

          {mode === 'safe' && contract.status === 'draft' && (
            <p className="muted">Safe mode: review the plan, then Lock &amp; Start.</p>
          )}

          <div className="row">
            <button className="btn-primary" onClick={() => void window.biscuit.contract.lock()}>
              {contract.status === 'draft' ? 'Lock & Start' : 'Re-lock'}
            </button>
            <button onClick={() => void window.biscuit.contract.clear()}>Clear</button>
            <button className="link-btn" onClick={() => setShowJson((v) => !v)}>
              {showJson ? 'Hide JSON' : 'Edit as JSON'}
            </button>
          </div>

          {showJson && (
            <>
              <p className="muted">Advanced: edit the raw contract, then lock the edited version.</p>
              <textarea rows={12} value={draft} onChange={(e) => setDraft(e.target.value)} />
              {error && <p style={{ color: 'var(--danger)' }}>{error}</p>}
              <div className="row">
                <button onClick={lockEdited}>Lock edited contract</button>
              </div>
            </>
          )}
        </>
      )}

      <div className="section-title">Preview a plan</div>
      <p className="muted">Generate a plan from any prompt without starting a task.</p>
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
            setPreviewContract(await window.biscuit.contract.generate(preview.trim()))
          }}
        >
          Generate preview
        </button>
      </div>
      {previewContract && <pre className="todo-box">{contractToTodo(previewContract)}</pre>}
    </div>
  )
}
