import { describe, it, expect, beforeEach, vi } from 'vitest'
import type { ActionProposal, AgentView, PageSignature, RuntimeUpdate } from '@shared/types'

// Queue of proposals the mocked model returns, in order.
const model = vi.hoisted(() => ({ queue: [] as ActionProposal[] }))
vi.mock('../src/main/agent/llm', () => ({
  llmJson: async () => model.queue.shift() ?? { kind: 'done', message: 'queue empty' },
  LlmError: class extends Error {}
}))

import { AgentRuntime, type RuntimeDeps } from '../src/main/agent/runtime'

const VIEW: AgentView = {
  tabId: 't1',
  url: 'https://e.com/',
  title: 'T',
  generation: 1,
  capturedAt: 0,
  headings: [],
  elements: [
    {
      ref: '@e1',
      role: 'link',
      tag: 'a',
      name: 'x',
      href: 'https://e.com/y',
      state: { visible: true, enabled: true, inViewport: true },
      box: { x: 0, y: 0, width: 10, height: 10 }
    }
  ],
  text: '',
  truncated: false
}

const SIG: PageSignature = {
  url: 'https://e.com/',
  title: 'T',
  textLength: 1,
  textHash: 1,
  interactiveCount: 1,
  alerts: [],
  invalidFields: 0,
  busy: false,
  capturedAt: 0
}

function makeRuntime(execImpl?: () => Promise<{ ok: boolean; detail: string }>) {
  const emits: RuntimeUpdate[] = []
  const tabs = {
    list: () => [
      {
        id: 't1',
        title: 'T',
        url: 'https://e.com/',
        canGoBack: false,
        canGoForward: false,
        isLoading: false,
        active: true
      }
    ],
    summaries: () => '*t1: T <https://e.com/>',
    getAgentView: async () => VIEW
  }
  const deps: RuntimeDeps = {
    tabs: tabs as never,
    getLlmConfig: () => ({ provider: 'openai', model: 'm', baseUrl: 'b', apiKey: null }),
    getMode: () => 'auto',
    getContract: () => null,
    execute: execImpl ?? (async () => ({ ok: true, detail: 'did it' })),
    getSignature: async () => SIG,
    requestApproval: async () => true,
    cancelApprovals: () => {},
    log: () => {},
    emit: (u) => emits.push(u)
  }
  return { runtime: new AgentRuntime(deps), emits }
}

beforeEach(() => {
  model.queue = []
})

describe('AgentRuntime loop guards', () => {
  it('stops when the same action repeats with no progress', async () => {
    for (let i = 0; i < 6; i++) model.queue.push({ kind: 'clickRef', ref: '@e1' })
    const { runtime, emits } = makeRuntime()
    await runtime.start('do a thing')
    const err = emits.find((e) => e.status === 'error')
    expect(err?.message).toMatch(/repeated/i)
  })

  it('stops after too many consecutive failures', async () => {
    // Distinct URLs so the repeat-guard doesn't fire first; each execute fails.
    for (let i = 0; i < 6; i++) model.queue.push({ kind: 'openUrl', url: `https://e.com/${i}` })
    const { runtime, emits } = makeRuntime(async () => ({ ok: false, detail: 'nope' }))
    await runtime.start('do a thing')
    const err = emits.find((e) => e.status === 'error')
    expect(err?.message).toMatch(/failed in a row/i)
  })

  it('carries a caution into done when the prior action did not visibly change anything', async () => {
    // clickRef "succeeds" but before/after signatures are identical → no-op
    // warning → done should be cautioned.
    model.queue.push({ kind: 'clickRef', ref: '@e1' })
    model.queue.push({ kind: 'done', message: 'all set' })
    const { runtime, emits } = makeRuntime()
    await runtime.start('do a thing')
    const done = emits.find((e) => e.status === 'done')
    expect(done?.message).toMatch(/all set/)
    expect(done?.message).toMatch(/caution/i)
  })
})
