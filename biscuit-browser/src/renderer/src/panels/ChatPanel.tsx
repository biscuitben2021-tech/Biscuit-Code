import { useEffect, useRef, useState } from 'react'
import { useBiscuit } from '../state/store'

export function ChatPanel(): JSX.Element {
  const { chat, runtime } = useBiscuit()
  const [text, setText] = useState('')
  const logRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const el = logRef.current
    if (el) el.scrollTop = el.scrollHeight
  }, [chat.length])

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
          <div className="msg system">
            Ask Biscuit to research or do something on the web. A Task Contract is generated from your
            request before any action runs.
          </div>
        )}
        {chat.map((m) => (
          <div key={m.id} className={`msg ${m.role}`}>
            {m.text}
          </div>
        ))}
      </div>
      {runtime && <div className="muted" style={{ padding: '0 10px 6px' }}>status: {runtime.status}</div>}
      <div className="composer">
        <textarea
          value={text}
          placeholder="Message Biscuit…  (Enter to send, Shift+Enter for newline)"
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault()
              send()
            }
          }}
        />
        <button onClick={send} disabled={!text.trim()}>
          Send
        </button>
      </div>
    </>
  )
}
