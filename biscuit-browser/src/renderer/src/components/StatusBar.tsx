import { describeMode } from '@shared/types'
import { useBiscuit } from '../state/store'

/**
 * Always-visible status strip at the top of the side panel: current permission
 * mode, contract status, agent status, and the most recent risk level — plus a
 * persistent Stop button whenever the agent is running.
 */
export function StatusBar(): JSX.Element {
  const { mode, contract, runtime, logs, approvals } = useBiscuit()
  const agent = runtime?.status ?? 'idle'
  const running = agent === 'running' || agent === 'awaiting-approval'

  // Most relevant risk: a pending approval's, else the latest gate decision's.
  const lastRisk = approvals[approvals.length - 1]?.gate.risk ?? [...logs].reverse().find((l) => l.risk)?.risk
  const riskLabel = lastRisk ?? '—'
  const riskClass =
    lastRisk === 'low'
      ? 'dec-allow'
      : lastRisk === 'medium'
        ? 'dec-ask'
        : lastRisk === 'high'
          ? 'dec-block'
          : ''

  return (
    <div className="statusbar">
      <span className={`sb-chip mode-${mode}`} title={describeMode(mode)}>
        {mode === 'bypass' ? 'BYPASS ⚠' : mode}
      </span>
      <span className="sb-item">
        contract <b>{contract.status}</b>
      </span>
      <span className="sb-item">
        agent <b>{agent}</b>
      </span>
      <span className="sb-item">
        risk <b className={riskClass}>{riskLabel}</b>
      </span>
      {running && (
        <button
          className="btn-danger sb-stop"
          title="Emergency stop — halt the agent and cancel pending actions"
          onClick={() => void window.biscuit.runtime.stop()}
        >
          ■ Stop
        </button>
      )}
    </div>
  )
}
