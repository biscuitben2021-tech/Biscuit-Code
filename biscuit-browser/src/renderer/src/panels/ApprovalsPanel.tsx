import type { ActionProposal } from '@shared/types'
import { useBiscuit } from '../state/store'

function describe(p: ActionProposal): string {
  switch (p.kind) {
    case 'openUrl':
      return `Open ${p.url}`
    case 'clickRef':
      return `Click ${p.ref}`
    case 'typeRef':
      return `Type "${p.text ?? ''}" into ${p.ref}`
    case 'scroll':
      return `Scroll ${p.direction ?? 'down'} ${p.pages ?? 1}`
    default:
      return p.kind
  }
}

export function ApprovalsPanel(): JSX.Element {
  const { approvals } = useBiscuit()
  return (
    <div className="panel-scroll">
      {approvals.length === 0 && (
        <p className="muted">
          No pending approvals. When the agent proposes an action the gate flags (or that the contract
          requires confirming), it appears here.
        </p>
      )}
      {approvals.map((a) => (
        <div key={a.id} className="approval">
          <div>
            <b>{describe(a.proposal)}</b>
          </div>
          {a.proposal.rationale && <div className="muted">why: {a.proposal.rationale}</div>}
          <div className="muted">
            gate: {a.gate.decision} · risk {a.gate.risk} — {a.gate.reason}
          </div>
          <div className="row" style={{ marginTop: 6 }}>
            <button onClick={() => void window.biscuit.approvals.respond(a.id, true)}>Approve</button>
            <button onClick={() => void window.biscuit.approvals.respond(a.id, false)}>Deny</button>
          </div>
        </div>
      ))}
    </div>
  )
}
