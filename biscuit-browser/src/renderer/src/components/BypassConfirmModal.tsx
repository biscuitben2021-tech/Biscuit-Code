import { useEffect, useRef, useState } from 'react'

const PHRASE = 'ENABLE BYPASS'

interface Props {
  open: boolean
  onCancel: () => void
  onConfirm: () => void
}

/**
 * A deliberately high-friction confirmation for arming Bypass mode. The user
 * must type the exact phrase, acknowledging that the agent will act with no
 * per-action approvals. The emergency Stop and the full action log stay active
 * in Bypass; only the gate prompts are skipped.
 */
export function BypassConfirmModal({ open, onCancel, onConfirm }: Props): JSX.Element | null {
  const [text, setText] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (open) {
      setText('')
      // focus after paint
      const t = window.setTimeout(() => inputRef.current?.focus(), 30)
      return () => window.clearTimeout(t)
    }
  }, [open])

  if (!open) return null
  const ready = text.trim() === PHRASE

  return (
    <div className="modal-backdrop" role="dialog" aria-modal="true" aria-label="Enable Bypass mode">
      <div className="modal danger">
        <div className="modal-title">⚠ Enable Bypass mode?</div>
        <p>
          In <b>Bypass</b> the agent runs <b>every action with no approval and no Task Contract</b> —
          including high-risk actions like logins, payments, sending messages, and deletions.
        </p>
        <ul className="modal-list">
          <li>The emergency <b>Stop</b> button stays active.</li>
          <li>Every action is still written to the <b>Action Log</b>.</li>
          <li>Pressing <b>Stop</b> automatically drops back to Assisted.</li>
        </ul>
        <p className="muted">
          Type <code>{PHRASE}</code> to confirm you understand the risk.
        </p>
        <input
          ref={inputRef}
          value={text}
          placeholder={PHRASE}
          spellCheck={false}
          autoCapitalize="characters"
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && ready) onConfirm()
            if (e.key === 'Escape') onCancel()
          }}
        />
        <div className="row" style={{ marginTop: 10, justifyContent: 'flex-end' }}>
          <button onClick={onCancel}>Cancel</button>
          <button className="btn-danger" disabled={!ready} onClick={onConfirm}>
            Enable Bypass
          </button>
        </div>
      </div>
    </div>
  )
}
