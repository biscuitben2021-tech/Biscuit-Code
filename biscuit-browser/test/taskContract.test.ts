import { describe, it, expect } from 'vitest'
import { normalizeActions, parseContract, fallbackContract } from '../src/main/agent/taskContract'

describe('normalizeActions', () => {
  it('keeps only known actions, lowercased and de-duplicated', () => {
    expect(normalizeActions(['open', 'READ', 'Open', 'frobnicate', 'click'])).toEqual(['open', 'read', 'click'])
  })

  it('ignores non-arrays and non-string entries', () => {
    expect(normalizeActions('open')).toEqual([])
    expect(normalizeActions(null)).toEqual([])
    expect(normalizeActions([1, 2, 'submit', {}])).toEqual(['submit'])
  })
})

describe('parseContract', () => {
  it('builds a contract from well-formed model output', () => {
    const c = parseContract(
      {
        goal: 'Find the latest Rust version',
        allowed_actions: ['open', 'read', 'search', 'scroll', 'click'],
        requires_user_confirmation: ['type'],
        blocked_without_user_override: ['payment', 'login']
      },
      'find rust version'
    )
    expect(c.goal).toBe('Find the latest Rust version')
    expect(c.allowed_actions).toEqual(['open', 'read', 'search', 'scroll', 'click'])
    expect(c.requires_user_confirmation).toEqual(['type'])
    expect(c.blocked_without_user_override).toEqual(['payment', 'login'])
  })

  it('falls back to the safe default when no usable allowed_actions are present', () => {
    const c = parseContract({ goal: 'whatever', allowed_actions: ['bogus'] }, 'do a thing')
    expect(c).toEqual(fallbackContract('do a thing'))
    // Safe default: research-friendly, money/auth/destructive blocked.
    expect(c.allowed_actions).toContain('read')
    expect(c.blocked_without_user_override).toEqual(
      expect.arrayContaining(['login', 'payment', 'send', 'delete', 'settings'])
    )
  })

  it('uses the prompt as the goal when the model omits one', () => {
    const c = parseContract({ allowed_actions: ['open', 'read'] }, '  research the topic  ')
    expect(c.goal).toBe('research the topic')
  })

  it('caps an over-long goal at 400 chars', () => {
    const long = 'x'.repeat(1000)
    const c = parseContract({ goal: long, allowed_actions: ['read'] }, 'p')
    expect(c.goal.length).toBe(400)
  })

  it('strips unknown actions out of every bucket', () => {
    const c = parseContract(
      {
        goal: 'g',
        allowed_actions: ['open', 'hack'],
        requires_user_confirmation: ['type', 'nope'],
        blocked_without_user_override: ['payment', 'explode']
      },
      'p'
    )
    expect(c.allowed_actions).toEqual(['open'])
    expect(c.requires_user_confirmation).toEqual(['type'])
    expect(c.blocked_without_user_override).toEqual(['payment'])
  })
})

describe('fallbackContract', () => {
  it('provides a default goal when the prompt is empty', () => {
    expect(fallbackContract('').goal).toBe('Assist with the requested browser task')
  })
})
