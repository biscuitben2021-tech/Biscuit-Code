import { useEffect, useRef, useState } from 'react'
import type { ActionProposal, ChatMessage } from '@shared/types'
import { useBiscuit } from '../state/store'

const EXAMPLES = [
  'Summarize this page.',
  'Find the pricing page and compare the plans.',
  'Open the docs and find the install command.',
  'List every clickable action on this page.'
]

function avatar(role: ChatMessage['role']): string {
  return role === 'user' ? 'You' : role === 'assistant' ? '🍪' : 'ⓘ'
}
function speaker(role: ChatMessage['role']): string {
  return role === 'user' ? 'You' : role === 'assistant' ? 'Biscuit' : 'System'
}
function describeProposal(p: ActionProposal): string {
  switch (p.kind) {
    case 'openUrl':
      return `Open ${p.url}`
    case 'clickRef':
      return `Click ${p.ref}`
    case 'typeRef':
      return `Type “${p.text ?? ''}” into ${p.ref}`
    case 'scroll':
      return `Scroll ${p.direction ?? 'down'} ${p.pages ?? 1}`
    default:
      return p.kind
  }
}

export function ChatPanel(): JSX.Element {
  const { chat, runtime, approvals } = useBiscuit()
  const [text, setText] = useState('')
  const logRef = useRef<HTMLDivElement>(null)

  const status = runtime?.status
  // Only show the spinner while running with nothing waiting on the user.
  const working = status === 'running' || (status === 'awaiting-approval' && approvals.length === 0)

  useEffect(() => {
    const el = logRef.current
    if (el) el.scrollTop = el.scrollHeight
  }, [chat.length, runtime?.message, working, approvals.length])

  const send = (): void => {
    const value = text.trim()
    if (!value) return
    void window.biscuit.chat.send(value)
    setText('')
  }

  return (
    <>
      <div className="chat-log" ref={logRef}>
        {chat.length === 0 && (
          <div className="chat-empty">
            Ask Biscuit to research or do something on the web. A Task Contract is generated from your request
            before any action runs.
            <div className="examples">
              <span className="muted">Try:</span>
              {EXAMPLES.map((ex) => (
                <button key={ex} className="example-chip" onClick={() => setText(ex)}>
                  {ex}
                </button>
              ))}
            </div>
            <div className="examples">
              <span className="muted">No API key?</span>
              <button className="example-chip" onClick={() => void window.biscuit.demo.run()}>
                ▶ Run keyless demo
              </button>
            </div>
          </div>
        )}

        {chat.map((m) => (
          <div key={m.id} className={`turn turn-${m.role}`}>
            <div className={`avatar avatar-${m.role}`}>{avatar(m.role)}</div>
            <div className="turn-body">
              <div className="turn-name">{speaker(m.role)}</div>
              <div className="turn-text">{m.text}</div>
            </div>
          </div>
        ))}

        {working && (
          <div className="turn turn-assistant">
            <div className="avatar avatar-assistant">🍪</div>
            <div className="turn-body">
              <div className="turn-name">Biscuit</div>
              <div className="turn-status">
                <span className="dots" aria-hidden="true">
                  <i />
                  <i />
                  <i />
                </span>
                <span>{runtime?.message || 'Working…'}</span>
              </div>
              <button
                className="btn-danger turn-stop"
                title="Emergency stop — halt the agent and cancel pending actions"
                onClick={() => void window.biscuit.runtime.stop()}
              >
                ■ Stop
              </button>
            </div>
          </div>
        )}

        {approvals.map((a) => (
          <div key={a.id} className="turn turn-assistant approval-turn">
            <div className="avatar avatar-assistant">🍪</div>
            <div className="turn-body">
              <div className="turn-name">Approval needed</div>
              <div className="turn-text">
                <b>{describeProposal(a.proposal)}</b>
              </div>
              {a.proposal.rationale && <div className="muted approval-why">{a.proposal.rationale}</div>}
              <div className="muted approval-why">
                gate: {a.gate.decision} · {a.gate.risk} risk — {a.gate.reason}
              </div>
              <div className="approval-actions">
                <button
                  className="btn-approve"
                  onClick={() => void window.biscuit.approvals.respond(a.id, true)}
                >
                  Approve
                </button>
                <button onClick={() => void window.biscuit.approvals.respond(a.id, false)}>Deny</button>
              </div>
            </div>
          </div>
        ))}
      </div>

      <div className="composer">
        <div className="composer-box">
          <textarea
            value={text}
            placeholder="Message Biscuit…  (Enter to send, Shift+Enter for newline)"
            rows={1}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault()
                send()
              }
            }}
          />
          <button className="send-btn" onClick={send} disabled={!text.trim()} title="Send">
            ↑
          </button>
        </div>
      </div>
    </>
  )
}
