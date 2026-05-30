interface Props {
  onClose: (openSettings: boolean) => void
}

/**
 * First-run onboarding. Explains the permission modes and points the user at
 * either Settings (to pick a provider) or Demo mode (no key required). Shown
 * once; the "seen" flag is stored in localStorage by the parent.
 */
export function Onboarding({ onClose }: Props): JSX.Element {
  return (
    <div className="modal-backdrop" role="dialog" aria-modal="true" aria-label="Welcome to Biscuit Browser">
      <div className="modal">
        <div className="modal-title accent">🍪 Welcome to Biscuit Browser</div>
        <p>
          An AI browser that reads pages as a structured <b>Agent View</b> (not screenshots) and runs every
          action through a <b>Task Contract</b> and a permission gate. Choose how much control you want:
        </p>
        <ul className="modal-list">
          <li>
            <b>Safe</b> — review &amp; lock the contract; ask before most actions.
          </li>
          <li>
            <b>Assisted</b> <span className="muted">(default)</span> — auto low/medium-risk; ask before
            high-risk.
          </li>
          <li>
            <b>Auto</b> — act inside the contract; ask only for high-risk actions.
          </li>
          <li>
            <b className="danger-text">Bypass (Danger)</b> — no prompts, no contract. Expert only; must be
            armed with a typed confirmation.
          </li>
        </ul>
        <p className="muted">
          Next, pick a model provider in <b>Settings</b> — or try <b>Demo mode</b> in the Chat panel (no API
          key needed) to see the Agent View, a Task Contract, and the Action Gate first.
        </p>
        <div className="row" style={{ marginTop: 10, justifyContent: 'flex-end' }}>
          <button onClick={() => onClose(false)}>Explore first</button>
          <button className="btn-primary" onClick={() => onClose(true)}>
            Open Settings
          </button>
        </div>
      </div>
    </div>
  )
}
