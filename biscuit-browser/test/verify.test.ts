import { describe, it, expect } from 'vitest'
import type { ActionProposal, PageSignature } from '@shared/types'
import { isMutating, shouldVerify, verifyAction } from '../src/main/agent/verify'

function sig(over: Partial<PageSignature> = {}): PageSignature {
  return {
    url: 'https://a.com/',
    title: 'A',
    textLength: 100,
    textHash: 111,
    interactiveCount: 5,
    alerts: [],
    invalidFields: 0,
    busy: false,
    capturedAt: 0,
    ...over
  }
}

const click: ActionProposal = { kind: 'clickRef', ref: '@e1' }
const type: ActionProposal = { kind: 'typeRef', ref: '@e1', text: 'x' }
const open: ActionProposal = { kind: 'openUrl', url: 'https://b.com' }
const scrollA: ActionProposal = { kind: 'scroll', direction: 'down' }

describe('isMutating / shouldVerify', () => {
  it('classifies action kinds correctly', () => {
    expect(isMutating('clickRef')).toBe(true)
    expect(isMutating('scroll')).toBe(true)
    expect(isMutating('refreshAgentView')).toBe(false)

    expect(shouldVerify('clickRef')).toBe(true)
    expect(shouldVerify('typeRef')).toBe(true)
    expect(shouldVerify('openUrl')).toBe(true)
    // Scroll mutates the viewport but isn't expected to change page content.
    expect(shouldVerify('scroll')).toBe(false)
    expect(shouldVerify('screenshot')).toBe(false)
  })
})

describe('verifyAction', () => {
  it('flags a no-op click (no observable change)', () => {
    const v = verifyAction(sig(), sig(), click)
    expect(v.changed).toBe(false)
    expect(v.ok).toBe(true)
    expect(v.warnings.join(' ')).toMatch(/no visible change/i)
  })

  it('recognizes navigation', () => {
    const v = verifyAction(sig(), sig({ url: 'https://a.com/next' }), click)
    expect(v.changed).toBe(true)
    expect(v.ok).toBe(true)
    expect(v.summary).toMatch(/navigated/i)
    expect(v.warnings).toHaveLength(0)
  })

  it('recognizes content change via text hash + length', () => {
    const v = verifyAction(sig(), sig({ textHash: 999, textLength: 240 }), click)
    expect(v.changed).toBe(true)
    expect(v.warnings).toHaveLength(0)
  })

  it('recognizes a title change', () => {
    const v = verifyAction(sig(), sig({ title: 'B' }), open)
    expect(v.changed).toBe(true)
    expect(v.summary).toMatch(/title/i)
  })

  it('marks the action failed when a new error banner appears', () => {
    const v = verifyAction(sig(), sig({ alerts: ['Your card was declined'] }), click)
    expect(v.ok).toBe(false)
    expect(v.warnings.join(' ')).toMatch(/card was declined/i)
  })

  it('does not re-flag a pre-existing alert', () => {
    const before = sig({ alerts: ['Heads up'] })
    const after = sig({ alerts: ['Heads up'] })
    const v = verifyAction(before, after, click)
    expect(v.ok).toBe(true)
  })

  it('marks a submit/click failed when more fields become invalid', () => {
    const v = verifyAction(sig({ invalidFields: 0 }), sig({ invalidFields: 2 }), click)
    expect(v.ok).toBe(false)
    expect(v.warnings.join(' ')).toMatch(/validation failed/i)
  })

  it('does NOT flag a rising invalid count for typeRef (partial input toggles :invalid)', () => {
    // textHash changes so it is detected as "changed"; the invalid bump must not
    // be reported as a failure for typing.
    const v = verifyAction(sig({ invalidFields: 0, textHash: 1 }), sig({ invalidFields: 1, textHash: 2 }), type)
    expect(v.ok).toBe(true)
    expect(v.warnings.join(' ')).not.toMatch(/validation failed/i)
  })

  it('reports a still-loading page instead of crying no-op', () => {
    const v = verifyAction(sig(), sig({ busy: true }), click)
    expect(v.warnings.join(' ')).toMatch(/still loading|rendering/i)
    expect(v.warnings.join(' ')).not.toMatch(/no visible change/i)
  })

  it('does not warn "no change" for scroll (not an expects-change action)', () => {
    const v = verifyAction(sig(), sig(), scrollA)
    expect(v.changed).toBe(false)
    expect(v.ok).toBe(true)
    expect(v.warnings).toHaveLength(0)
  })

  it('detects typing via the control-value hash even though innerText is unchanged', () => {
    // The signature folds control values into textHash, so a real typeRef shows up.
    const v = verifyAction(sig({ textHash: 111 }), sig({ textHash: 222 }), type)
    expect(v.changed).toBe(true)
  })
})
