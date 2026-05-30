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

export interface Classification {
  contractAction: ContractActionName | null // null = control-flow / inspection (not gated by contract)
  risk: ActionRisk
  sensitive: boolean
  note: string
}

// ── Page-context risk ────────────────────────────────────────────────────────
// A vague "Continue" or "Confirm" button means something very different on a
// checkout page than on a docs page. The gate derives the page context from the
// (untrusted) Agent View — url, title, headings, and a slice of visible text —
// and uses it to escalate otherwise-ambiguous clicks. This is heuristic and
// intentionally errs toward asking.
export type PageContextKind = 'payment' | 'login' | 'account'

export interface PageContext {
  domain: string
  kinds: PageContextKind[]
  sensitive: boolean
}

function domainOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, '')
  } catch {
    return ''
  }
}

const RE = {
  // Page-context signals. Deliberately specific so we don't flag a docs page
  // that merely says "pay attention" or "forgot password" as a money/auth flow.
  payment:
    /\b(payment|pay now|pay with|checkout|billing|credit ?card|debit ?card|card ?number|cvv|cvc|order summary|place order|order now|purchase|shopping cart|add to cart|invoice|donate|subscription|transfer funds|wire transfer|online banking|account number|routing number)\b/,
  login:
    /\b(sign ?in|log ?in|login|two-?factor|2fa|one-?time (code|password)|verification code|authenticat|create (your |an )?account|reset password)\b/,
  account:
    /\b(account settings|profile settings|manage account|delete account|deactivate|close (your )?account|privacy settings|security settings|api keys?|payment methods?|connected accounts)\b/,
  // Action words found on a clicked control's label.
  destructive:
    /\b(delete|remove|erase|destroy|deactivate|close account|unsubscribe|revoke|reset|empty trash|clear (all|data|everything|cache)|wipe|discard|terminate|purge|cancel (subscription|membership|plan))\b/,
  payAction:
    /\b(pay|buy|purchase|checkout|place order|order now|add to cart|donate|subscribe|transfer|withdraw|authori[sz]e|wire|remit|payment|send money|send payment)\b/,
  sendAction: /\b(send|share|post|publish|tweet|message|invite|email|submit application)\b/,
  authAction:
    /\b(sign ?in|log ?in|login|register|sign ?up|create account|continue with (google|apple|github|facebook|email))\b/,
  submitAction: /\b(submit|confirm|place|complete|verify|apply now|save changes|approve|accept terms)\b/,
  vagueAction: /\b(continue|next|proceed|agree|accept|allow|ok|okay|done|finish|got it|apply|save|update)\b/
} as const

/** Derive the sensitivity context of the current page from the Agent View. */
export function analyzePageContext(view: AgentView | null): PageContext {
  if (!view) return { domain: '', kinds: [], sensitive: false }
  const hay = [
    view.url || '',
    view.title || '',
    (view.headings || []).map((h) => h.text).join(' '),
    (view.text || '').slice(0, 1500)
  ]
    .join('  ')
    .toLowerCase()
  const kinds: PageContextKind[] = []
  if (RE.payment.test(hay)) kinds.push('payment')
  if (RE.login.test(hay)) kinds.push('login')
  if (RE.account.test(hay)) kinds.push('account')
  return { domain: domainOf(view.url), kinds, sensitive: kinds.length > 0 }
}

/** Contract action that best matches the current sensitive context. */
function contextAction(ctx: PageContext): ContractActionName {
  if (ctx.kinds.includes('payment')) return 'payment'
  if (ctx.kinds.includes('login')) return 'login'
  if (ctx.kinds.includes('account')) return 'settings'
  return 'submit'
}

function findElement(view: AgentView | null, ref?: string): AgentElement | undefined {
  if (!view || !ref) return undefined
  return view.elements.find((e) => e.ref === ref || e.ref === `@${ref.replace(/^@/, '')}`)
}

/** Classify a clickRef using the target element + the page context. */
function classifyClick(el: AgentElement | undefined, ctx: PageContext): Classification {
  const name = (el?.name ?? '').toLowerCase()

  if (el?.sensitive)
    return { contractAction: 'submit', risk: 'high', sensitive: true, note: 'click on sensitive control' }

  if (RE.destructive.test(name))
    return {
      contractAction: 'delete',
      risk: 'high',
      sensitive: false,
      note: `destructive click "${name.slice(0, 40)}"`
    }
  if (RE.payAction.test(name))
    return {
      contractAction: 'payment',
      risk: 'high',
      sensitive: false,
      note: `payment click "${name.slice(0, 40)}"`
    }
  if (RE.sendAction.test(name))
    return {
      contractAction: 'send',
      risk: 'high',
      sensitive: false,
      note: `send/share click "${name.slice(0, 40)}"`
    }
  if (RE.authAction.test(name))
    return {
      contractAction: 'login',
      risk: 'high',
      sensitive: false,
      note: `auth click "${name.slice(0, 40)}"`
    }

  if (RE.submitAction.test(name) || el?.role === 'submit' || el?.type === 'submit') {
    return {
      contractAction: contextAction(ctx),
      risk: 'high',
      sensitive: false,
      note: ctx.sensitive ? `submit/confirm in ${ctx.kinds.join('/')} context` : 'submit/confirm click'
    }
  }

  // Vague labels ("Continue", "Next", "Agree") are high-risk in a sensitive
  // context (they advance a payment/login/account flow) and medium otherwise.
  if (RE.vagueAction.test(name)) {
    if (ctx.sensitive)
      return {
        contractAction: contextAction(ctx),
        risk: 'high',
        sensitive: false,
        note: `vague action "${name.slice(0, 24)}" in ${ctx.kinds.join('/')} context`
      }
    return {
      contractAction: 'submit',
      risk: 'medium',
      sensitive: false,
      note: `vague action "${name.slice(0, 24)}"`
    }
  }

  // On a sensitive page (payment/login/account), escalate clicks on ACTION-style
  // controls — buttons, submits, custom widgets, or links that don't simply
  // navigate. These can advance a checkout/login flow even with an innocuous
  // label, so they require confirmation ('submit'/high → ask) rather than
  // auto-running. A plain navigation link (an <a> with an http href) stays low:
  // clicking it just loads a page, which is itself re-evaluated when it opens —
  // so general browsing of a site that merely has a "Sign in" header still works.
  const isNavLink = (el?.tag === 'a' || el?.role === 'link') && !!el?.href && /^https?:/i.test(el.href)
  if (ctx.sensitive && !isNavLink) {
    return {
      contractAction: 'submit',
      risk: 'high',
      sensitive: false,
      note: `action in ${ctx.kinds.join('/')} context — confirm`
    }
  }

  // A navigation link that leaves the current site is medium-risk: it's still
  // just navigation (re-evaluated on the destination), but crossing to an
  // unknown domain is worth a heads-up — Safe mode asks, Assisted/Auto allow.
  if (isNavLink) {
    const dest = domainOf(el!.href!)
    if (dest && ctx.domain && dest !== ctx.domain) {
      return {
        contractAction: 'open',
        risk: 'medium',
        sensitive: false,
        note: `navigate to external domain ${dest}`
      }
    }
    return { contractAction: 'click', risk: 'low', sensitive: false, note: 'navigation link' }
  }

  const isButton = el?.role === 'button' || el?.tag === 'button'
  return { contractAction: 'click', risk: 'low', sensitive: false, note: isButton ? 'button click' : 'click' }
}

/** Map a proposed action to a contract action name + a base risk level. */
export function classify(proposal: ActionProposal, view: AgentView | null): Classification {
  const ctx = analyzePageContext(view)
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
    case 'clickRef':
      return classifyClick(findElement(view, proposal.ref), ctx)
    case 'typeRef': {
      const el = findElement(view, proposal.ref)
      if (el?.sensitive)
        return { contractAction: 'login', risk: 'high', sensitive: true, note: 'type into sensitive field' }
      // Typing on a payment page may be an unlabeled card/secret field — confirm.
      if (ctx.kinds.includes('payment'))
        return {
          contractAction: 'type',
          risk: 'high',
          sensitive: true,
          note: 'typing in a payment context (possible card/secret field)'
        }
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
 * target element/page context, and sensitive fields. It returns allow / ask /
 * block.
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
  if (
    (proposal.kind === 'clickRef' || proposal.kind === 'typeRef') &&
    !findElement(agentView, proposal.ref)
  ) {
    return ask('high', 'target @ref is not in the current Agent View — refresh and retry')
  }

  // Contract checks (when a contract is locked).
  if (contract) {
    if (contract.blocked_without_user_override.includes(contractAction)) {
      return block(
        risk === 'low' ? 'medium' : risk,
        `'${contractAction}' is blocked by the task contract (needs an explicit user override / contract edit) — ${note}`
      )
    }
    if (contract.requires_user_confirmation.includes(contractAction)) {
      return ask(risk, `task contract requires confirmation for '${contractAction}' — ${note}`)
    }
    if (!contract.allowed_actions.includes(contractAction)) {
      return ask(risk, `'${contractAction}' is not in the task contract's allowed actions — ${note}`)
    }
  }

  // Sensitive fields always escalate, regardless of mode (except bypass above).
  if (sensitive) return ask('high', `${note} — sensitive, requires confirmation`)

  // Mode + risk policy for contract-permitted actions.
  switch (mode) {
    case 'safe':
      return risk === 'low'
        ? allow(risk, 'safe: low-risk allowed')
        : ask(risk, `safe: confirm ${risk}-risk ${contractAction} — ${note}`)
    case 'assisted':
      return risk === 'high'
        ? ask(risk, `assisted: confirm high-risk ${contractAction} — ${note}`)
        : allow(risk, 'assisted: low/medium allowed')
    case 'auto':
      // TODO(phase-later): per-action "auto-approve high-risk" config toggle.
      return risk === 'high'
        ? ask(risk, `auto: confirm high-risk ${contractAction} — ${note}`)
        : allow(risk, 'auto: acting inside contract')
    default:
      return ask(risk, 'unknown mode — confirming to be safe')
  }
}
