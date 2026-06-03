import { useEffect, useReducer } from 'react'
import type {
  ApprovalRequest,
  ChatMessage,
  ContractState,
  LogEntry,
  PermissionMode,
  RuntimeUpdate,
  Settings,
  TabState
} from '@shared/types'
import { contractToTodo } from '../contractText'

export interface AppState {
  tabs: TabState[]
  activeTabId: string | null
  mode: PermissionMode
  contract: ContractState
  logs: LogEntry[]
  approvals: ApprovalRequest[]
  chat: ChatMessage[]
  runtime: RuntimeUpdate | null
  settings: Settings | null
}

const initial: AppState = {
  tabs: [],
  activeTabId: null,
  mode: 'assisted',
  contract: { status: 'none', contract: null, prompt: '', createdAt: 0, lockedAt: null },
  logs: [],
  approvals: [],
  chat: [],
  runtime: null,
  settings: null
}

let state: AppState = initial
const listeners = new Set<() => void>()

export function getState(): AppState {
  return state
}

function set(patch: Partial<AppState>): void {
  state = { ...state, ...patch }
  for (const l of listeners) l()
}

function subscribe(l: () => void): () => void {
  listeners.add(l)
  return () => listeners.delete(l)
}

let seq = 0
function sysMessage(role: ChatMessage['role'], text: string): ChatMessage {
  seq += 1
  return { id: `r-${Date.now()}-${seq}`, role, text, ts: Date.now() }
}

// Avoid re-posting the same task plan to the chat on every contract event.
let lastContractSig = ''

let inited = false
export async function init(): Promise<void> {
  if (inited) return
  inited = true
  const b = window.biscuit

  b.onTabsChanged((tabs) => set({ tabs, activeTabId: tabs.find((t) => t.active)?.id ?? null }))
  b.onModeChanged((mode) => set({ mode }))
  b.onContractChanged((contract) => {
    set({ contract })
    // Surface the plan in the chat as a plain-text to-do (not JSON) when it is
    // proposed (draft) or locked, de-duplicated so it isn't re-posted.
    if ((contract.status === 'draft' || contract.status === 'locked') && contract.contract) {
      const sig = `${contract.status}|${JSON.stringify(contract.contract)}`
      if (sig !== lastContractSig) {
        lastContractSig = sig
        const head =
          contract.status === 'draft'
            ? '📋 Here’s the task plan — review it in the Contract tab, then Lock & Start:'
            : '📋 Task plan locked — starting:'
        set({
          chat: [
            ...getState().chat,
            sysMessage('assistant', `${head}\n\n${contractToTodo(contract.contract)}`)
          ]
        })
      }
    } else if (contract.status === 'none') {
      lastContractSig = ''
    }
  })
  b.onLogAppended((entry) => set({ logs: [...getState().logs, entry].slice(-1000) }))
  b.onApprovalRequested((req) => set({ approvals: [...getState().approvals, req] }))
  b.onApprovalResolved(({ id }) => set({ approvals: getState().approvals.filter((a) => a.id !== id) }))
  b.onChatMessage((m) => set({ chat: [...getState().chat, m] }))
  b.onRuntimeUpdate((update) => {
    // Keep the chat clean (ChatGPT-style): per-step "running" updates drive the
    // live "working…" indicator only; just the agent's final/awaiting messages
    // become assistant turns. The Action Log keeps the full step-by-step trace.
    const surface = update.status === 'done' || update.status === 'error' || update.status === 'stopped'
    const chat =
      surface && update.message
        ? [...getState().chat, sysMessage('assistant', update.message)]
        : getState().chat
    set({ runtime: update, chat })
  })

  const [tabs, mode, contract, settings, logs] = await Promise.all([
    b.tabs.list(),
    b.mode.get(),
    b.contract.get(),
    b.settings.get(),
    b.log.list()
  ])
  set({
    tabs,
    activeTabId: tabs.find((t) => t.active)?.id ?? null,
    mode,
    contract,
    settings,
    logs
  })
}

/** Subscribe a component to the whole store. Simple + fine for this app size. */
export function useBiscuit(): AppState {
  const [, force] = useReducer((x: number) => x + 1, 0)
  useEffect(() => subscribe(force), [])
  return getState()
}

export { set as patchState }
