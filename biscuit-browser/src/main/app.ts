import { BrowserWindow, ipcMain } from 'electron'
import type {
  ActionProposal,
  ApprovalRequest,
  ChatMessage,
  ContractState,
  GateResult,
  PermissionMode,
  Settings,
  SettingsUpdate,
  TaskContract
} from '@shared/types'
import { IPC, type ViewBounds } from '@shared/ipc'
import { ActionLog } from './log'
import { SettingsStore } from './settings/store'
import { TabManager } from './tabs'
import { AgentRuntime } from './agent/runtime'
import { generateContract } from './agent/taskContract'
import { executeProposal } from './actions/execute'
import type { LlmConfig } from './agent/llm'

let chatSeq = 0
function chatMessage(role: ChatMessage['role'], text: string): ChatMessage {
  chatSeq += 1
  return { id: `msg-${Date.now()}-${chatSeq}`, role, text, ts: Date.now() }
}

/**
 * Central orchestrator that lives in the main process. It owns permission mode,
 * the locked Task Contract, approvals, the tab manager, and the agent runtime,
 * and exposes everything to the renderer through a strict IPC surface.
 */
export class App {
  private mode: PermissionMode
  private contract: ContractState = {
    status: 'none',
    contract: null,
    prompt: '',
    createdAt: 0,
    lockedAt: null
  }
  private pendingPrompt: string | null = null
  private readonly approvals = new Map<string, (ok: boolean) => void>()
  private apSeq = 0

  private readonly log: ActionLog
  private readonly tabs: TabManager
  private readonly runtime: AgentRuntime

  constructor(
    private readonly win: BrowserWindow,
    private readonly settings: SettingsStore
  ) {
    this.mode = this.settings.get().defaultMode
    this.log = new ActionLog((entry) => this.send(IPC.EVT_LOG_APPENDED, entry))
    this.tabs = new TabManager(win, {
      onTabsChanged: () => this.send(IPC.EVT_TABS_CHANGED, this.tabs.list())
    })
    this.runtime = new AgentRuntime({
      tabs: this.tabs,
      getLlmConfig: () => this.llmConfig(),
      getMode: () => this.mode,
      getContract: () => (this.contract.status === 'locked' ? this.contract.contract : null),
      execute: (proposal) => executeProposal(this.tabs, proposal),
      requestApproval: (proposal, gate) => this.requestApproval(proposal, gate),
      cancelApprovals: () => this.cancelApprovals(),
      log: (entry) => void this.log.add(entry),
      emit: (update) => this.send(IPC.EVT_RUNTIME_UPDATE, update)
    })
    this.registerIpc()
    this.tabs.create()
  }

  // ── helpers ─────────────────────────────────────────────────────────────
  private send(channel: string, payload: unknown): void {
    if (!this.win.isDestroyed()) this.win.webContents.send(channel, payload)
  }

  private llmConfig(): LlmConfig {
    const s = this.settings.get()
    return { provider: s.provider, model: s.model, baseUrl: s.baseUrl, apiKey: this.settings.getApiKey() }
  }

  private setMode(mode: PermissionMode): void {
    this.mode = mode
    this.log.add({ type: 'system', mode, message: `permission mode set to ${mode}` })
    this.send(IPC.EVT_MODE_CHANGED, mode)
  }

  private setContract(next: ContractState): void {
    this.contract = next
    this.log.add({ type: 'contract', message: `contract ${next.status}: ${next.contract?.goal ?? '(cleared)'}` })
    this.send(IPC.EVT_CONTRACT_CHANGED, next)
  }

  // ── contract + task flow ─────────────────────────────────────────────────
  private async startTask(prompt: string): Promise<void> {
    const clean = prompt.trim()
    if (!clean) return
    this.send(IPC.EVT_CHAT_MESSAGE, chatMessage('user', clean))

    // A new prompt must never replace the locked contract of a running task.
    if (this.runtime.isRunning()) {
      this.send(IPC.EVT_CHAT_MESSAGE, chatMessage('system', 'A task is already running. Press Stop before starting a new one.'))
      return
    }

    // Bypass: expert mode — no contract is generated or enforced; the gate is
    // skipped (logs + emergency stop still apply). Run immediately.
    if (this.mode === 'bypass') {
      void this.runtime.start(clean)
      return
    }

    const contract = await generateContract(this.llmConfig(), clean)

    if (this.mode === 'safe') {
      // Safe mode: never lock without explicit user approval. Hold the prompt.
      this.pendingPrompt = clean
      this.setContract({ status: 'draft', contract, prompt: clean, createdAt: Date.now(), lockedAt: null })
      this.send(IPC.EVT_RUNTIME_UPDATE, {
        status: 'awaiting-approval',
        message: 'Review and lock the Task Contract (Contract tab) to begin.'
      })
      this.send(IPC.EVT_CHAT_MESSAGE, chatMessage('system', 'Safe mode: review and lock the Task Contract to start.'))
      return
    }

    // Assisted / Auto: auto-lock the contract, then run.
    this.setContract({ status: 'locked', contract, prompt: clean, createdAt: Date.now(), lockedAt: Date.now() })
    void this.runtime.start(clean)
  }

  private lockContract(edited?: TaskContract): void {
    const contract = edited ?? this.contract.contract
    if (!contract) return
    this.setContract({
      status: 'locked',
      contract,
      prompt: this.contract.prompt,
      createdAt: this.contract.createdAt || Date.now(),
      lockedAt: Date.now()
    })
    if (this.pendingPrompt) {
      const prompt = this.pendingPrompt
      this.pendingPrompt = null
      void this.runtime.start(prompt)
    }
  }

  // ── approvals ─────────────────────────────────────────────────────────────
  private requestApproval(proposal: ActionProposal, gate: GateResult): Promise<boolean> {
    this.apSeq += 1
    const id = `ap-${this.apSeq}`
    const request: ApprovalRequest = { id, proposal, gate }
    this.send(IPC.EVT_APPROVAL_REQUESTED, request)
    return new Promise<boolean>((resolve) => this.approvals.set(id, resolve))
  }

  private resolveApproval(id: string, approved: boolean): void {
    const resolve = this.approvals.get(id)
    if (resolve) {
      this.approvals.delete(id)
      resolve(approved)
      this.send(IPC.EVT_APPROVAL_RESOLVED, { id })
    }
  }

  private cancelApprovals(): void {
    for (const [id, resolve] of this.approvals) {
      resolve(false)
      this.send(IPC.EVT_APPROVAL_RESOLVED, { id })
    }
    this.approvals.clear()
  }

  // ── IPC registration ───────────────────────────────────────────────────────
  private registerIpc(): void {
    const h = ipcMain.handle.bind(ipcMain)

    // Tabs
    h(IPC.TAB_CREATE, (_e, url?: string) => this.tabs.create(url))
    h(IPC.TAB_CLOSE, (_e, id: string) => this.tabs.close(id))
    h(IPC.TAB_ACTIVATE, (_e, id: string) => this.tabs.activate(id))
    h(IPC.TAB_NAVIGATE, (_e, id: string, url: string) => this.tabs.navigate(id, url))
    h(IPC.TAB_BACK, (_e, id: string) => this.tabs.back(id))
    h(IPC.TAB_FORWARD, (_e, id: string) => this.tabs.forward(id))
    h(IPC.TAB_RELOAD, (_e, id: string) => this.tabs.reload(id))
    h(IPC.TAB_LIST, () => this.tabs.list())

    // Native view bounds
    h(IPC.VIEW_SET_BOUNDS, (_e, bounds: ViewBounds) => this.tabs.setBounds(bounds))

    // Agent View + manual actions
    h(IPC.AGENT_VIEW_GET, (_e, tabId?: string) => this.tabs.getAgentView(tabId))
    h(IPC.AGENT_VIEW_REFRESH, (_e, tabId?: string) => this.tabs.refreshAgentView(tabId))
    h(IPC.ACTION_RUN, async (_e, proposal: ActionProposal) => {
      // Inspection only. Page-mutating actions must go through the gated agent
      // runtime — they are never runnable directly from the renderer.
      if (proposal.kind !== 'screenshot' && proposal.kind !== 'refreshAgentView') {
        return { ok: false, detail: `'${proposal.kind}' is not allowed here; use the agent (gated) for actions` }
      }
      const result = await executeProposal(this.tabs, proposal)
      this.log.add({ type: 'action', action: proposal.kind, message: `manual: ${result.detail}` })
      return result
    })

    // Task Contract
    h(IPC.CONTRACT_GENERATE, async (_e, prompt: string) => generateContract(this.llmConfig(), prompt))
    h(IPC.CONTRACT_GET, () => this.contract)
    h(IPC.CONTRACT_LOCK, (_e, contract?: TaskContract) => {
      this.lockContract(contract)
      return this.contract
    })
    h(IPC.CONTRACT_CLEAR, () => {
      this.pendingPrompt = null
      this.setContract({ status: 'none', contract: null, prompt: '', createdAt: 0, lockedAt: null })
      return this.contract
    })

    // Mode
    h(IPC.MODE_GET, () => this.mode)
    h(IPC.MODE_SET, (_e, mode: PermissionMode) => {
      this.setMode(mode)
      return this.mode
    })

    // Logs
    h(IPC.LOG_LIST, () => this.log.list())

    // Approvals
    h(IPC.APPROVAL_RESPOND, (_e, id: string, approved: boolean) => {
      this.resolveApproval(id, approved)
      return true
    })

    // Settings (the raw key never leaves main beyond LLM calls)
    h(IPC.SETTINGS_GET, (): Settings => this.settings.get())
    h(IPC.SETTINGS_SAVE, (_e, update: SettingsUpdate): Settings => {
      const prevDefault = this.settings.get().defaultMode
      const saved = this.settings.save(update)
      // Adopt the new default only if it actually changed — saving an API key
      // or model must not silently reset a manually-chosen session mode.
      if (!this.runtime.isRunning() && saved.defaultMode !== prevDefault) {
        this.setMode(saved.defaultMode)
      }
      return saved
    })

    // Chat starts/continues a task. (RUNTIME_STOP is the emergency stop.)
    h(IPC.CHAT_SEND, (_e, message: string) => this.startTask(message))
    h(IPC.RUNTIME_STOP, () => {
      this.runtime.stop()
      this.cancelApprovals()
      return true
    })
  }

  destroy(): void {
    this.runtime.stop()
    this.tabs.destroy()
  }
}
