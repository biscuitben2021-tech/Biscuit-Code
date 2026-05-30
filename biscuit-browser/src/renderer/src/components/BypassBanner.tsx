interface Props {
  running: boolean
}

/**
 * A bright, persistent banner shown whenever Bypass mode is armed, so it can
 * never be forgotten. Offers a one-click drop back to Assisted and mirrors the
 * emergency Stop.
 */
export function BypassBanner({ running }: Props): JSX.Element {
  return (
    <div className="bypass-banner" role="alert">
      <span className="bypass-dot" aria-hidden="true" />
      <span className="bypass-text">
        <b>BYPASS MODE ARMED</b> — the agent acts with no approvals and no contract.
      </span>
      <span className="bypass-actions">
        <button
          className="btn-danger"
          disabled={!running}
          title="Emergency stop — halt the agent and cancel pending actions"
          onClick={() => void window.biscuit.runtime.stop()}
        >
          ■ Stop
        </button>
        <button
          title="Drop back to Assisted (asks before high-risk actions)"
          onClick={() => void window.biscuit.mode.set('assisted')}
        >
          Reset to Assisted
        </button>
      </span>
    </div>
  )
}
