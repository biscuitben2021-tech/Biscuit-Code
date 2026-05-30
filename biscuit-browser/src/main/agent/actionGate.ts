import type {
  ActionProposal,
  ActionRisk,
  AgentElement,
  AgentView,
  ContractActionName,
  GateResult,
  PermissionMode,
  TaskContract
} from '@shared/types'

export interface GateInput {
  proposal: ActionProposal
  mode: PermissionMode
  contract: TaskContract | null
  agentView: AgentView | null
}

interface Classification {
  contractAction: ContractActionName | null // null = control-flow / inspection (not gated by contract)
  risk: ActionRisk
  sensitive: boolean
  note: string
}

function findElement(view: AgentView | null, ref?: string): AgentElement | undefined {
  if (!view || !ref) return undefined
  return view.elements.find((e) => e.ref === ref || e.ref === `@${ref.replace(/^@/, '')}`)
}

/** Map a proposed action to a contract action name + a base risk level. */
function classify(proposal: ActionProposal, view: AgentView | null): Classification {
  switch (proposal.kind) {
    case 'openUrl':
      return { contractAction: 'open', risk: 'low', sensitive: false, note: 'navigate' }
    case 'scroll':
      return { contractAction: 'scroll', risk: 'low', sensitive: false, note: 'scroll' }
    case 'refreshAgentView':
    case 'screenshot':
      return { contractAction: null, risk: 'low', sensitive: false, note: 'inspection' }
    case 'done':
    case 'ask':
      return { contractAction: null, risk: 'low', sensitive: false, note: 'control-flow' }
    case 'clickRef': {
      const el = findElement(view, proposal.ref)
      const submitLike =
        el?.role === 'submit' || el?.type === 'submit' || /submit|pay|buy|checkout|delete|send/i.test(el?.name ?? '')
      if (el?.sensitive) return { contractAction: 'submit', risk: 'high', sensitive: true, note: 'click on sensitive control' }
      if (submitLike) return { contractAction: 'submit', risk: 'high', sensitive: false, note: 'submit-like click' }
      return { contractAction: 'click', risk: 'low', sensitive: false, note: 'click' }
    }
    case 'typeRef': {
      const el = findElement(view, proposal.ref)
      if (el?.sensitive) return { contractAction: 'login', risk: 'high', sensitive: true, note: 'type into sensitive field' }
      return { contractAction: 'type', risk: 'medium', sensitive: false, note: 'type' }
    }
  }
}

function allow(risk: ActionRisk, reason: string): GateResult {
  return { decision: 'allow', risk, reason }
}
function ask(risk: ActionRisk, reason: string): GateResult {
  return { decision: 'ask', risk, reason }
}
function block(risk: ActionRisk, reason: string): GateResult {
  return { decision: 'block', risk, reason }
}

/**
 * The ActionGate. Every proposed action passes through here before execution
 * (unless the mode is Bypass — the caller skips the gate, but still logs). The
 * gate considers: permission mode, action risk, the locked Task Contract, the
 * target element/domain, and sensitive fields. It returns allow / ask / block.
 */
export function evaluate(input: GateInput): GateResult {
  const { proposal, mode, contract, agentView } = input
  const { contractAction, risk, sensitive, note } = classify(proposal, agentView)

  // Bypass: expert mode. The runtime normally skips the gate entirely in bypass,
  // but if it does call us we allow (the decision is still logged by the caller).
  if (mode === 'bypass') return allow(risk, 'bypass mode')

  // Control-flow / inspection actions are never contract-gated.
  if (contractAction === null) return allow('low', `${note} is always permitted`)

  // Ref-based actions require a fresh Agent View. If the ref is not present
  // (view failed to capture, or the ref is stale/hallucinated), don't silently
  // treat it as a low-risk click/type — ask and force a refresh.
  if ((proposal.kind === 'clickRef' || proposal.kind === 'typeRef') && !findElement(agentView, proposal.ref)) {
    return ask('high', 'target @ref is not in the current Agent View — refresh and retry')
  }

  // Contract checks (when a contract is locked).
  if (contract) {
    if (contract.blocked_without_user_override.includes(contractAction)) {
      return block(
        risk === 'low' ? 'medium' : risk,
        `'${contractAction}' is blocked by the task contract (needs an explicit user override / contract edit)`
      )
    }
    if (contract.requires_user_confirmation.includes(contractAction)) {
      return ask(risk, `task contract requires confirmation for '${contractAction}'`)
    }
    if (!contract.allowed_actions.includes(contractAction)) {
      return ask(risk, `'${contractAction}' is not in the task contract's allowed actions`)
    }
  }

  // Sensitive fields always escalate, regardless of mode (except bypass above).
  if (sensitive) return ask('high', `${note} — sensitive field requires confirmation`)

  // Mode + risk policy for contract-permitted actions.
  switch (mode) {
    case 'safe':
      return risk === 'low' ? allow(risk, 'safe: low-risk allowed') : ask(risk, `safe: confirm ${risk}-risk ${contractAction}`)
    case 'assisted':
      return risk === 'high' ? ask(risk, `assisted: confirm high-risk ${contractAction}`) : allow(risk, 'assisted: low/medium allowed')
    case 'auto':
      // TODO(phase-later): per-action "auto-approve high-risk" config toggle.
      return risk === 'high' ? ask(risk, `auto: confirm high-risk ${contractAction}`) : allow(risk, 'auto: acting inside contract')
    default:
      return ask(risk, 'unknown mode — confirming to be safe')
  }
}
