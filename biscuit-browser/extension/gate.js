// Pure Action Gate for the extension — adapts the risk model from
// src/main/agent/actionGate.ts. Classifies a proposed action by risk using the
// element label + page context, then a permission mode maps risk to a decision.

const HIGH_RISK_LABEL =
  /\b(pay|buy|purchase|order|checkout|delete|remove|send|transfer|confirm|submit|sign\s?in|log\s?in|login|register|subscribe|place order)\b/i
const SENSITIVE_CONTEXT =
  /\b(payment|checkout|password|sign\s?in|log\s?in|billing|credit card|debit card|bank|wire|invoice)\b/i

/** @returns {{ action: string, risk: 'low'|'medium'|'high' }} */
export function classify(proposal, view) {
  const kind = proposal.kind
  if (kind === 'openUrl' || kind === 'refreshAgentView' || kind === 'scroll' || kind === 'screenshot') {
    return { action: kind === 'openUrl' ? 'open' : 'read', risk: 'low' }
  }
  const el = (view && view.elements ? view.elements : []).find((e) => e.ref === proposal.ref)
  const label = ((el && el.label) || '').toLowerCase()
  const ctx = `${(view && view.title) || ''} ${((view && view.text) || '').slice(0, 1500)}`.toLowerCase()
  const sensitiveCtx = SENSITIVE_CONTEXT.test(ctx)

  if (kind === 'typeRef') {
    if (el && el.sensitive) return { action: 'type', risk: 'high' }
    return { action: 'type', risk: sensitiveCtx ? 'high' : 'medium' }
  }
  if (kind === 'clickRef') {
    if (HIGH_RISK_LABEL.test(label)) return { action: 'submit', risk: 'high' }
    if (sensitiveCtx) return { action: 'click', risk: 'high' }
    return { action: 'click', risk: 'medium' }
  }
  return { action: kind, risk: 'medium' }
}

/** Map (mode, risk) -> 'allow' | 'ask'. Mirrors the CLI/browser permission tiers. */
export function decide(mode, risk) {
  switch (mode) {
    case 'bypass':
      return 'allow'
    case 'safe':
      return risk === 'low' ? 'allow' : 'ask'
    case 'assisted':
    case 'auto':
      return risk === 'high' ? 'ask' : 'allow'
    default:
      return 'ask'
  }
}
