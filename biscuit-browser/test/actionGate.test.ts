import { describe, it, expect } from 'vitest'
import type { ActionProposal, AgentElement, AgentView, PermissionMode, TaskContract } from '@shared/types'
import { analyzePageContext, classify, evaluate } from '../src/main/agent/actionGate'

// ── builders ──────────────────────────────────────────────────────────────
let refSeq = 0
function el(over: Partial<AgentElement> = {}): AgentElement {
  refSeq += 1
  return {
    ref: over.ref ?? `@e${refSeq}`,
    role: 'button',
    tag: 'button',
    name: '',
    state: { visible: true, enabled: true, inViewport: true },
    box: { x: 0, y: 0, width: 20, height: 10 },
    ...over
  }
}
function view(elements: AgentElement[] = [], over: Partial<AgentView> = {}): AgentView {
  return {
    tabId: 't1',
    url: 'https://example.com/',
    title: 'Example',
    generation: 1,
    capturedAt: 0,
    headings: [],
    elements,
    text: '',
    truncated: false,
    ...over
  }
}

// Default contract = the conservative research default.
const DEFAULT_CONTRACT: TaskContract = {
  goal: 'g',
  allowed_actions: ['open', 'read', 'search', 'scroll', 'click'],
  requires_user_confirmation: ['type', 'submit', 'upload', 'download'],
  blocked_without_user_override: ['login', 'payment', 'send', 'delete', 'settings']
}
// Permissive contract for exercising the mode/risk policy in isolation.
const PERMISSIVE: TaskContract = {
  goal: 'g',
  allowed_actions: ['open', 'read', 'search', 'scroll', 'click', 'type', 'submit'],
  requires_user_confirmation: [],
  blocked_without_user_override: []
}

const ALL_MODES: PermissionMode[] = ['safe', 'assisted', 'auto', 'bypass']

function gate(
  proposal: ActionProposal,
  mode: PermissionMode,
  v: AgentView | null,
  contract: TaskContract | null = DEFAULT_CONTRACT
) {
  return evaluate({ proposal, mode, contract, agentView: v })
}

// ── page context ────────────────────────────────────────────────────────────
describe('analyzePageContext', () => {
  it('detects a payment context', () => {
    const ctx = analyzePageContext(
      view([], { url: 'https://shop.test/checkout', text: 'Order summary — place order' })
    )
    expect(ctx.kinds).toContain('payment')
    expect(ctx.sensitive).toBe(true)
  })

  it('detects a login context', () => {
    const ctx = analyzePageContext(view([], { title: 'Sign in', text: 'Enter your password to log in' }))
    expect(ctx.kinds).toContain('login')
  })

  it('detects an account-settings context', () => {
    const ctx = analyzePageContext(view([], { text: 'Manage account · delete account · security settings' }))
    expect(ctx.kinds).toContain('account')
  })

  it('treats a neutral docs page as non-sensitive', () => {
    const ctx = analyzePageContext(
      view([], { url: 'https://doc.rust-lang.org/', text: 'The Rust Programming Language' })
    )
    expect(ctx.sensitive).toBe(false)
    expect(ctx.kinds).toEqual([])
  })

  it('does not over-match incidental words ("pay attention", "forgot password")', () => {
    const ctx = analyzePageContext(
      view([], {
        title: 'Tips',
        text: 'Pay attention to detail. If you forgot password hints, write them down.'
      })
    )
    expect(ctx.sensitive).toBe(false)
  })

  it('extracts a bare domain', () => {
    expect(analyzePageContext(view([], { url: 'https://www.example.com/x/y' })).domain).toBe('example.com')
  })

  it('is safe on a null view', () => {
    expect(analyzePageContext(null)).toEqual({ domain: '', kinds: [], sensitive: false })
  })
})

// ── classification ───────────────────────────────────────────────────────────
describe('classify', () => {
  it('maps navigation/scroll/inspection appropriately', () => {
    expect(classify({ kind: 'openUrl', url: 'https://x' }, null).contractAction).toBe('open')
    expect(classify({ kind: 'scroll', direction: 'down' }, null).contractAction).toBe('scroll')
    expect(classify({ kind: 'refreshAgentView' }, null).contractAction).toBeNull()
    expect(classify({ kind: 'screenshot' }, null).contractAction).toBeNull()
    expect(classify({ kind: 'done' }, null).contractAction).toBeNull()
  })

  it('classifies a plain link click as low-risk', () => {
    const v = view([
      el({ ref: '@e1', role: 'link', tag: 'a', name: 'Documentation', href: 'https://example.com/docs' })
    ])
    const c = classify({ kind: 'clickRef', ref: '@e1' }, v)
    expect(c.contractAction).toBe('click')
    expect(c.risk).toBe('low')
  })

  it('maps destructive button labels to delete/high', () => {
    const v = view([el({ ref: '@e1', name: 'Delete account' })])
    const c = classify({ kind: 'clickRef', ref: '@e1' }, v)
    expect(c.contractAction).toBe('delete')
    expect(c.risk).toBe('high')
  })

  it('maps a "Pay now" button to payment/high', () => {
    const v = view([el({ ref: '@e1', name: 'Pay now' })])
    expect(classify({ kind: 'clickRef', ref: '@e1' }, v).contractAction).toBe('payment')
  })

  it('escalates a vague "Continue" inside a payment context', () => {
    const v = view([el({ ref: '@e1', name: 'Continue' })], {
      url: 'https://shop/checkout',
      text: 'place order — billing'
    })
    const c = classify({ kind: 'clickRef', ref: '@e1' }, v)
    expect(c.contractAction).toBe('payment')
    expect(c.risk).toBe('high')
  })

  it('treats a vague "Continue" on a neutral page as medium submit', () => {
    const v = view([el({ ref: '@e1', name: 'Continue' })], { text: 'just a blog post' })
    const c = classify({ kind: 'clickRef', ref: '@e1' }, v)
    expect(c.contractAction).toBe('submit')
    expect(c.risk).toBe('medium')
  })

  it('flags clicks on sensitive controls', () => {
    const v = view([el({ ref: '@e1', sensitive: true, name: 'card' })])
    const c = classify({ kind: 'clickRef', ref: '@e1' }, v)
    expect(c.sensitive).toBe(true)
    expect(c.risk).toBe('high')
  })

  it('flags typing into sensitive fields', () => {
    const v = view([el({ ref: '@e1', role: 'textbox', tag: 'input', sensitive: true })])
    const c = classify({ kind: 'typeRef', ref: '@e1', text: 'secret' }, v)
    expect(c.contractAction).toBe('login')
    expect(c.sensitive).toBe(true)
  })

  it('escalates typing in a payment context even without a sensitive flag', () => {
    const v = view([el({ ref: '@e1', role: 'textbox', tag: 'input', name: 'number' })], {
      url: 'https://shop/checkout',
      text: 'card number cvv'
    })
    const c = classify({ kind: 'typeRef', ref: '@e1', text: '4111' }, v)
    expect(c.sensitive).toBe(true)
    expect(c.risk).toBe('high')
  })

  it('flags a navigation link to an external domain as medium', () => {
    const v = view(
      [el({ ref: '@e1', role: 'link', tag: 'a', name: 'Sponsor', href: 'https://other.example/x' })],
      {
        url: 'https://mysite.test/page'
      }
    )
    const c = classify({ kind: 'clickRef', ref: '@e1' }, v)
    expect(c.contractAction).toBe('open')
    expect(c.risk).toBe('medium')
  })

  it('keeps a same-domain navigation link low-risk', () => {
    const v = view(
      [el({ ref: '@e1', role: 'link', tag: 'a', name: 'Read more', href: 'https://mysite.test/next' })],
      {
        url: 'https://mysite.test/page'
      }
    )
    const c = classify({ kind: 'clickRef', ref: '@e1' }, v)
    expect(c.contractAction).toBe('click')
    expect(c.risk).toBe('low')
  })
})

describe('evaluate — external navigation', () => {
  const extLink = view(
    [el({ ref: '@e1', role: 'link', tag: 'a', name: 'Sponsor', href: 'https://other.example/x' })],
    { url: 'https://mysite.test/page' }
  )
  it('asks in safe mode, allows in assisted/auto (medium-risk navigation)', () => {
    expect(gate({ kind: 'clickRef', ref: '@e1' }, 'safe', extLink).decision).toBe('ask')
    expect(gate({ kind: 'clickRef', ref: '@e1' }, 'assisted', extLink).decision).toBe('allow')
    expect(gate({ kind: 'clickRef', ref: '@e1' }, 'auto', extLink).decision).toBe('allow')
  })
})

// ── gate decisions ────────────────────────────────────────────────────────────
describe('evaluate — contract enforcement', () => {
  it('blocks contract-blocked actions (Pay now under the default contract)', () => {
    const v = view([el({ ref: '@e1', name: 'Pay now' })], {
      url: 'https://shop/checkout',
      text: 'place order'
    })
    const r = gate({ kind: 'clickRef', ref: '@e1' }, 'auto', v)
    expect(r.decision).toBe('block')
  })

  it('blocks a vague "Continue" on a checkout page (mapped to payment)', () => {
    const v = view([el({ ref: '@e1', name: 'Continue' })], {
      url: 'https://shop/checkout',
      text: 'order summary place order billing'
    })
    expect(gate({ kind: 'clickRef', ref: '@e1' }, 'auto', v).decision).toBe('block')
  })

  it('asks for actions requiring confirmation (type under default contract)', () => {
    const v = view([el({ ref: '@e1', role: 'textbox', tag: 'input', name: 'query' })])
    expect(gate({ kind: 'typeRef', ref: '@e1', text: 'hi' }, 'auto', v).decision).toBe('ask')
  })

  it('asks when an action is not in allowed_actions', () => {
    const onlyOpen: TaskContract = {
      goal: 'g',
      allowed_actions: ['open'],
      requires_user_confirmation: [],
      blocked_without_user_override: []
    }
    const v = view([
      el({ ref: '@e1', role: 'link', tag: 'a', name: 'docs', href: 'https://example.com/docs' })
    ])
    expect(gate({ kind: 'clickRef', ref: '@e1' }, 'auto', v, onlyOpen).decision).toBe('ask')
  })

  it('allows an in-contract low-risk click', () => {
    const v = view([
      el({ ref: '@e1', role: 'link', tag: 'a', name: 'docs', href: 'https://example.com/docs' })
    ])
    expect(gate({ kind: 'clickRef', ref: '@e1' }, 'assisted', v).decision).toBe('allow')
  })

  it('does not auto-allow an innocuously-labeled control on a sensitive page', () => {
    // "Go" matches no risky verb; on a checkout page it must still confirm
    // (submit/high → ask) rather than auto-run.
    const v = view([el({ ref: '@e1', role: 'button', tag: 'button', name: 'Go' })], {
      url: 'https://bank/checkout',
      text: 'order summary place order billing'
    })
    expect(gate({ kind: 'clickRef', ref: '@e1' }, 'auto', v).decision).toBe('ask')
  })

  it('still allows a plain navigation link on a sensitive page (general browsing intact)', () => {
    const v = view([el({ ref: '@e1', role: 'link', tag: 'a', name: 'Help', href: 'https://shop/help' })], {
      url: 'https://shop/checkout',
      text: 'order summary place order billing'
    })
    expect(gate({ kind: 'clickRef', ref: '@e1' }, 'auto', v).decision).toBe('allow')
  })

  it('blocks high-consequence verbs the old regex missed', () => {
    for (const name of [
      'Authorize payment',
      'Wire $10,000',
      'Empty trash',
      'Discard changes',
      'Cancel subscription'
    ]) {
      const v = view([el({ ref: '@e1', name })], { text: 'neutral page' })
      // Mapped to a blocked contract action by LABEL, regardless of page context.
      expect(gate({ kind: 'clickRef', ref: '@e1' }, 'auto', v).decision).toBe('block')
    }
  })

  it('escalates an innocuously-labeled BUTTON on a login page (not just nav links)', () => {
    const v = view([el({ ref: '@e1', role: 'button', tag: 'button', name: 'Yes' })], {
      title: 'Sign in',
      text: 'log in with your verification code'
    })
    expect(gate({ kind: 'clickRef', ref: '@e1' }, 'auto', v).decision).toBe('ask')
  })
})

describe('evaluate — ref integrity', () => {
  it('asks for a refresh when the @ref is missing from the view', () => {
    const v = view([el({ ref: '@e1', name: 'ok' })])
    const r = gate({ kind: 'clickRef', ref: '@e99' }, 'auto', v)
    expect(r.decision).toBe('ask')
    expect(r.reason).toMatch(/not in the current Agent View|refresh/i)
  })
})

describe('evaluate — mode/risk policy (permissive contract)', () => {
  it('safe asks for medium/high but allows low', () => {
    const link = view([
      el({ ref: '@e1', role: 'link', tag: 'a', name: 'docs', href: 'https://example.com/docs' })
    ])
    expect(gate({ kind: 'clickRef', ref: '@e1' }, 'safe', link, PERMISSIVE).decision).toBe('allow')

    const vague = view([el({ ref: '@e2', name: 'Continue' })], { text: 'neutral' })
    expect(gate({ kind: 'clickRef', ref: '@e2' }, 'safe', vague, PERMISSIVE).decision).toBe('ask')
  })

  it('assisted allows low/medium, asks high', () => {
    const vague = view([el({ ref: '@e1', name: 'Continue' })], { text: 'neutral' })
    expect(gate({ kind: 'clickRef', ref: '@e1' }, 'assisted', vague, PERMISSIVE).decision).toBe('allow')

    const submit = view([el({ ref: '@e2', name: 'Submit' })], { text: 'neutral' })
    // "Submit" → submit/high → assisted asks.
    expect(gate({ kind: 'clickRef', ref: '@e2' }, 'assisted', submit, PERMISSIVE).decision).toBe('ask')
  })

  it('auto allows low/medium, asks high', () => {
    const submit = view([el({ ref: '@e1', name: 'Submit' })], { text: 'neutral' })
    expect(gate({ kind: 'clickRef', ref: '@e1' }, 'auto', submit, PERMISSIVE).decision).toBe('ask')
  })
})

describe('evaluate — bypass + sensitive', () => {
  it('bypass allows everything (still logged by the caller)', () => {
    const v = view([el({ ref: '@e1', name: 'Delete account', sensitive: true })], { text: 'delete account' })
    expect(gate({ kind: 'clickRef', ref: '@e1' }, 'bypass', v).decision).toBe('allow')
  })

  it('sensitive fields escalate to ask even when the contract would allow them', () => {
    const v = view([el({ ref: '@e1', role: 'textbox', tag: 'input', sensitive: true })])
    // typeRef into a sensitive field maps to 'login'. Allow login so it passes
    // the contract checks — the only thing left to force an ask is the
    // sensitivity escalation itself.
    const allowLogin: TaskContract = {
      goal: 'g',
      allowed_actions: ['type', 'login'],
      requires_user_confirmation: [],
      blocked_without_user_override: []
    }
    const r = gate({ kind: 'typeRef', ref: '@e1', text: 'x' }, 'auto', v, allowLogin)
    expect(r.decision).toBe('ask')
    expect(r.reason).toMatch(/sensitive/i)
  })

  it('control-flow and inspection are always permitted', () => {
    for (const mode of ALL_MODES) {
      expect(gate({ kind: 'refreshAgentView' }, mode, view()).decision).toBe('allow')
    }
  })
})
