import { BrowserWindow, ipcMain } from 'electron'
import type {
  ActionProposal,
  ActionResult,
  AgentView,
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
import { generateContract, parseContract } from './agent/taskContract'
import { evaluate } from './agent/actionGate'
import { executeProposal } from './actions/execute'
import { startMcpServer, type McpServerHandle } from './mcp/server'
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

  // MCP server (exposes the browser to external AI agents) + the last Agent View
  // captured for a tool call, used to gate the next acting tool without
  // re-extracting (which would invalidate the @refs the agent just received).
  private mcp: McpServerHandle | null = null
  private lastMcpView: AgentView | null = null
  /** IPC channels this App registered, so destroy() can remove them. Without
   *  this, the macOS close-then-reactivate flow builds a second App whose
   *  registerIpc() throws "second handler" and the reopened window is dead. */
  private readonly ipcChannels: string[] = []

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
      getSignature: () => this.tabs.getSignature(),
      requestApproval: (proposal, gate) => this.requestApproval(proposal, gate),
      cancelApprovals: () => this.cancelApprovals(),
      log: (entry) => void this.log.add(entry),
      emit: (update) => this.send(IPC.EVT_RUNTIME_UPDATE, update)
    })
    this.registerIpc()
    this.tabs.create()
    void this.startMcp()
  }

  // ── MCP server ──────────────────────────────────────────────────────────
  private async startMcp(): Promise<void> {
    try {
      this.mcp = await startMcpServer({
        getAgentView: async () => {
          const v = await this.tabs.getAgentView()
          this.lastMcpView = v // cache for gating the next acting tool
          return v
        },
        runAction: (proposal) => this.mcpRunAction(proposal),
        screenshot: () => executeProposal(this.tabs, { kind: 'screenshot' }),
        listTabs: () => this.tabs.list(),
        newTab: (url) => this.tabs.create(url),
        status: () => ({
          mode: this.mode,
          contract: this.contract.status,
          running: this.runtime.isRunning()
        }),
        log: (message) => this.log.add({ type: 'system', message })
      })
      this.log.add({ type: 'system', message: `MCP server listening at ${this.mcp.url}` })
    } catch (err) {
      this.log.add({ type: 'system', message: `MCP server failed to start: ${(err as Error).message}` })
    }
  }

  /**
   * Run an acting proposal requested over MCP through the Action Gate, so an
   * external agent is held to the same permission model as the built-in one.
   * Uses the last Agent View captured by a tool call (not a fresh extract) so
   * the @refs the agent is using stay valid.
   */
  private async mcpRunAction(proposal: ActionProposal): Promise<ActionResult> {
    const mode = this.mode
    const contract = this.contract.status === 'locked' ? this.contract.contract : null
    if (mode !== 'bypass') {
      const gate = evaluate({ proposal, mode, contract, agentView: this.lastMcpView })
      this.log.add({
        type: 'gate',
        message: `mcp: ${gate.reason}`,
        action: proposal.kind,
        decision: gate.decision,
        risk: gate.risk,
        mode
      })
      if (gate.decision === 'block')
        return { ok: false, detail: `blocked by the Action Gate: ${gate.reason}` }
      if (gate.decision === 'ask') {
        this.send(IPC.EVT_RUNTIME_UPDATE, {
          status: 'awaiting-approval',
          message: `MCP action needs approval: ${proposal.kind}`
        })
        const approved = await this.requestApproval(proposal, gate, 180_000)
        if (!approved) return { ok: false, detail: `not approved by the user: ${gate.reason}` }
      }
    } else {
      this.log.add({
        type: 'gate',
        message: 'mcp: bypass (no prompt)',
        action: proposal.kind,
        decision: 'allow',
        risk: 'low',
        mode
      })
    }
    const result = await executeProposal(this.tabs, proposal)
    this.log.add({ type: 'action', action: proposal.kind, message: `mcp: ${result.detail}` })
    return result
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
    // Defense in depth: Bypass can only be armed when expert mode is enabled.
    // The renderer also gates this behind a typed-confirmation modal, but the
    // main process is the real authority.
    if (mode === 'bypass' && !this.settings.isExpertMode()) {
      this.log.add({ type: 'system', message: 'refused to enter Bypass mode: expert mode is not enabled' })
      this.send(IPC.EVT_MODE_CHANGED, this.mode) // re-affirm the real mode to the UI
      return
    }
    this.mode = mode
    this.log.add({ type: 'system', mode, message: `permission mode set to ${mode}` })
    this.send(IPC.EVT_MODE_CHANGED, mode)
  }

  private setContract(next: ContractState): void {
    this.contract = next
    this.log.add({
      type: 'contract',
      message: `contract ${next.status}: ${next.contract?.goal ?? '(cleared)'}`
    })
    this.send(IPC.EVT_CONTRACT_CHANGED, next)
  }

  // ── contract + task flow ─────────────────────────────────────────────────
  private async startTask(prompt: string): Promise<void> {
    const clean = prompt.trim()
    if (!clean) return
    this.send(IPC.EVT_CHAT_MESSAGE, chatMessage('user', clean))

    // A new prompt must never replace the locked contract of a running task.
    if (this.runtime.isRunning()) {
      this.send(
        IPC.EVT_CHAT_MESSAGE,
        chatMessage('system', 'A task is already running. Press Stop before starting a new one.')
      )
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
      this.send(
        IPC.EVT_CHAT_MESSAGE,
        chatMessage('system', 'Safe mode: review and lock the Task Contract to start.')
      )
      return
    }

    // Assisted / Auto: auto-lock the contract, then run.
    this.setContract({
      status: 'locked',
      contract,
      prompt: clean,
      createdAt: Date.now(),
      lockedAt: Date.now()
    })
    void this.runtime.start(clean)
  }

  private lockContract(edited?: TaskContract): void {
    // An edited contract comes straight from the renderer's raw-JSON editor and
    // may be any shape (e.g. `{}` or `5`). Normalize it through parseContract so
    // the locked contract always has the expected array fields — otherwise
    // contractToTodo()/the Action Gate dereference undefined arrays and crash
    // (white-screening the renderer and killing the task loop). Main is the
    // authority for contract shape.
    const contract = edited
      ? parseContract(edited as unknown as Record<string, unknown>, this.contract.prompt)
      : this.contract.contract
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
  private requestApproval(proposal: ActionProposal, gate: GateResult, timeoutMs?: number): Promise<boolean> {
    this.apSeq += 1
    const id = `ap-${this.apSeq}`
    const request: ApprovalRequest = { id, proposal, gate }
    this.send(IPC.EVT_APPROVAL_REQUESTED, request)
    return new Promise<boolean>((resolve) => {
      this.approvals.set(id, resolve)
      // MCP-initiated approvals time out (auto-deny) so a tool call can't hang
      // forever waiting on a human who isn't there.
      if (timeoutMs && timeoutMs > 0) {
        setTimeout(() => {
          if (this.approvals.has(id)) {
            this.approvals.delete(id)
            this.send(IPC.EVT_APPROVAL_RESOLVED, { id })
            resolve(false)
          }
        }, timeoutMs)
      }
    })
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
    // Record every channel so destroy() can remove it (handlers are global and
    // re-registering a still-registered channel throws).
    const h: typeof ipcMain.handle = (channel, listener) => {
      this.ipcChannels.push(channel)
      ipcMain.handle(channel, listener)
    }

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
        return {
          ok: false,
          detail: `'${proposal.kind}' is not allowed here; use the agent (gated) for actions`
        }
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
      // Revoking expert mode must immediately disarm any active Bypass session,
      // even mid-task — the runtime reads the mode each step, and dropping to
      // Assisted is always the safer direction.
      if (!saved.expertMode && this.mode === 'bypass') {
        this.log.add({ type: 'system', message: 'expert mode disabled — dropping Bypass back to Assisted' })
        this.setMode('assisted')
      }
      return saved
    })

    // Chat starts/continues a task. (RUNTIME_STOP is the emergency stop.)
    h(IPC.CHAT_SEND, (_e, message: string) => this.startTask(message))
    h(IPC.DEMO_RUN, () => this.runDemo())
    h(IPC.MCP_GET_INFO, () => ({
      running: this.mcp !== null,
      url: this.mcp?.url ?? '',
      port: this.mcp?.port ?? 0,
      token: this.mcp?.token ?? ''
    }))
    h(IPC.RUNTIME_STOP, () => {
      this.runtime.stop()
      this.cancelApprovals()
      // A panic-stop should also drop out of the riskiest mode. Bypass must be
      // re-armed deliberately afterwards.
      if (this.mode === 'bypass') this.setMode('assisted')
      return true
    })
  }

  // ── Demo mode (no API key) ──────────────────────────────────────────────
  /**
   * A scripted, model-free walkthrough so a first-time user can see the product
   * before configuring a provider. It locks a canned Task Contract and runs a
   * few synthetic actions through the REAL Action Gate (no model call, no
   * network, no real browser action) — showing allow / ask / block decisions in
   * the Action Log and Chat, plus the contract in the Contract tab.
   */
  private runDemo(): void {
    if (this.runtime.isRunning()) {
      this.send(
        IPC.EVT_CHAT_MESSAGE,
        chatMessage('system', 'Stop the running task before starting the demo.')
      )
      return
    }
    const say = (role: ChatMessage['role'], text: string): void =>
      this.send(IPC.EVT_CHAT_MESSAGE, chatMessage(role, text))

    say('user', '▶ Run the keyless demo')
    say(
      'system',
      'Demo mode — no model is called and no real browser action runs. This just shows the Task Contract and how the Action Gate classifies actions on a (fake) checkout page.'
    )

    const contract: TaskContract = {
      goal: 'Find a blue widget and read its price (demo)',
      allowed_actions: ['open', 'read', 'search', 'scroll', 'click'],
      requires_user_confirmation: ['type', 'submit'],
      blocked_without_user_override: ['login', 'payment', 'send', 'delete', 'settings']
    }
    this.setContract({
      status: 'locked',
      contract,
      prompt: '(demo) find a blue widget and read its price',
      createdAt: Date.now(),
      lockedAt: Date.now()
    })

    const demoView: AgentView = {
      tabId: 'demo',
      url: 'https://shop.example/checkout',
      title: 'Checkout — shop.example',
      generation: 1,
      capturedAt: Date.now(),
      headings: [{ level: 1, text: 'Checkout' }],
      elements: [
        {
          ref: '@e1',
          role: 'link',
          tag: 'a',
          name: 'Product details',
          href: 'https://shop.example/widget',
          state: { visible: true, enabled: true, inViewport: true },
          box: { x: 0, y: 0, width: 120, height: 20 }
        },
        {
          ref: '@e2',
          role: 'textbox',
          tag: 'input',
          name: 'Search products',
          state: { visible: true, enabled: true, inViewport: true },
          box: { x: 0, y: 0, width: 160, height: 24 }
        },
        {
          ref: '@e3',
          role: 'textbox',
          tag: 'input',
          name: 'Card number',
          sensitive: true,
          state: { visible: true, enabled: true, inViewport: true },
          box: { x: 0, y: 0, width: 200, height: 24 }
        },
        {
          ref: '@e4',
          role: 'button',
          tag: 'button',
          name: 'Continue',
          state: { visible: true, enabled: true, inViewport: true },
          box: { x: 0, y: 0, width: 90, height: 30 }
        },
        {
          ref: '@e5',
          role: 'button',
          tag: 'button',
          name: 'Pay now',
          state: { visible: true, enabled: true, inViewport: true },
          box: { x: 0, y: 0, width: 90, height: 30 }
        }
      ],
      text: 'Checkout. Order summary. Place order. Billing. Card number. Pay now.',
      truncated: false
    }

    say(
      'assistant',
      'Synthetic Agent View:\n  @e1 link "Product details"\n  @e2 textbox "Search products"\n  @e3 textbox "Card number" (SENSITIVE)\n  @e4 button "Continue"\n  @e5 button "Pay now"'
    )

    const steps: { p: ActionProposal; why: string }[] = [
      { p: { kind: 'scroll', direction: 'down' }, why: 'scroll the page' },
      { p: { kind: 'clickRef', ref: '@e1' }, why: 'click the "Product details" link' },
      { p: { kind: 'typeRef', ref: '@e2', text: 'blue widget' }, why: 'type "blue widget" into search' },
      { p: { kind: 'clickRef', ref: '@e4' }, why: 'click "Continue" (vague, on a checkout page)' },
      { p: { kind: 'clickRef', ref: '@e5' }, why: 'click "Pay now"' }
    ]
    for (const s of steps) {
      const gate = evaluate({ proposal: s.p, mode: 'assisted', contract, agentView: demoView })
      this.log.add({
        type: 'gate',
        message: `(demo) ${gate.reason}`,
        action: s.p.kind,
        decision: gate.decision,
        risk: gate.risk,
        mode: 'assisted'
      })
      const icon = gate.decision === 'allow' ? '✅ allow' : gate.decision === 'ask' ? '🔶 ask' : '🛑 block'
      say('assistant', `${icon} — ${s.why}\n   gate: ${gate.decision}/${gate.risk} — ${gate.reason}`)
    }

    say(
      'system',
      'End of demo. The contract is in the Contract tab and every decision is in the Action Log. Add a provider in Settings (or LM Studio, no key) to run the real agent.'
    )
    this.send(IPC.EVT_RUNTIME_UPDATE, { status: 'done', message: 'Demo complete.' })
  }

  destroy(): void {
    this.runtime.stop()
    void this.mcp?.close()
    this.tabs.destroy()
    // Remove our IPC handlers so a new App (macOS reactivate) can register them
    // again without throwing.
    for (const channel of this.ipcChannels) {
      ipcMain.removeHandler(channel)
    }
    this.ipcChannels.length = 0
  }
}
