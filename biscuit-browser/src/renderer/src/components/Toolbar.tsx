import { useEffect, useState } from 'react'
import { PERMISSION_MODES, describeMode, type PermissionMode, type RuntimeUpdate, type TabState } from '@shared/types'

interface Props {
  activeTab: TabState | null
  mode: PermissionMode
  runtime: RuntimeUpdate | null
}

export function Toolbar({ activeTab, mode, runtime }: Props): JSX.Element {
  const [address, setAddress] = useState('')
  const [editing, setEditing] = useState(false)

  // Keep the address bar in sync with the active tab unless the user is typing.
  useEffect(() => {
    if (!editing) setAddress(activeTab?.url ?? '')
  }, [activeTab?.url, activeTab?.id, editing])

  const go = (): void => {
    const value = address.trim()
    if (!value) return
    if (activeTab) void window.biscuit.tabs.navigate(activeTab.id, value)
    else void window.biscuit.tabs.create(value)
    setEditing(false)
  }

  const running = runtime?.status === 'running' || runtime?.status === 'awaiting-approval'

  return (
    <div className="toolbar">
      <div className="nav">
        <button
          title="Back"
          disabled={!activeTab?.canGoBack}
          onClick={() => activeTab && void window.biscuit.tabs.back(activeTab.id)}
        >
          ◀
        </button>
        <button
          title="Forward"
          disabled={!activeTab?.canGoForward}
          onClick={() => activeTab && void window.biscuit.tabs.forward(activeTab.id)}
        >
          ▶
        </button>
        <button title="Reload" onClick={() => activeTab && void window.biscuit.tabs.reload(activeTab.id)}>
          ⟳
        </button>
      </div>

      <div className="address">
        <input
          value={address}
          placeholder="Search or enter address"
          spellCheck={false}
          onFocus={() => setEditing(true)}
          onBlur={() => setEditing(false)}
          onChange={(e) => setAddress(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') go()
            if (e.key === 'Escape') {
              setEditing(false)
              setAddress(activeTab?.url ?? '')
            }
          }}
        />
      </div>

      <select
        className={`mode-chip mode-${mode}`}
        value={mode}
        title={describeMode(mode)}
        onChange={(e) => void window.biscuit.mode.set(e.target.value as PermissionMode)}
      >
        {PERMISSION_MODES.map((m) => (
          <option key={m} value={m}>
            {m}
          </option>
        ))}
      </select>

      <button
        className="estop"
        title="Emergency stop — halt the agent and cancel pending approvals"
        disabled={!running}
        onClick={() => void window.biscuit.runtime.stop()}
      >
        ■ Stop
      </button>
    </div>
  )
}
