import type {
  ActionProposal,
  ActionResult,
  AgentView,
  ApprovalRequest,
  ChatMessage,
  ContractState,
  LogEntry,
  PermissionMode,
  RuntimeUpdate,
  Settings,
  SettingsUpdate,
  TabState,
  TaskContract
} from './types'
import type { ViewBounds } from './ipc'

/**
 * The single, typed surface exposed to the renderer via contextBridge as
 * `window.biscuit`. The renderer can do nothing the main process does not
 * explicitly allow here — no raw Node, no ipcRenderer, no arbitrary channels.
 */
export interface BiscuitApi {
  tabs: {
    create(url?: string): Promise<string>
    close(id: string): Promise<void>
    activate(id: string): Promise<void>
    navigate(id: string, url: string): Promise<void>
    back(id: string): Promise<void>
    forward(id: string): Promise<void>
    reload(id: string): Promise<void>
    list(): Promise<TabState[]>
  }
  view: {
    setBounds(bounds: ViewBounds): Promise<void>
  }
  agentView: {
    get(tabId?: string): Promise<AgentView>
    refresh(tabId?: string): Promise<AgentView>
  }
  action: {
    run(proposal: ActionProposal): Promise<ActionResult>
  }
  contract: {
    generate(prompt: string): Promise<TaskContract>
    get(): Promise<ContractState>
    lock(contract?: TaskContract): Promise<ContractState>
    clear(): Promise<ContractState>
  }
  mode: {
    get(): Promise<PermissionMode>
    set(mode: PermissionMode): Promise<PermissionMode>
  }
  log: {
    list(): Promise<LogEntry[]>
  }
  approvals: {
    respond(id: string, approved: boolean): Promise<boolean>
  }
  settings: {
    get(): Promise<Settings>
    save(update: SettingsUpdate): Promise<Settings>
  }
  runtime: {
    stop(): Promise<boolean>
  }
  chat: {
    send(message: string): Promise<void>
  }

  // Event subscriptions (each returns an unsubscribe function).
  onTabsChanged(cb: (tabs: TabState[]) => void): () => void
  onContractChanged(cb: (contract: ContractState) => void): () => void
  onModeChanged(cb: (mode: PermissionMode) => void): () => void
  onLogAppended(cb: (entry: LogEntry) => void): () => void
  onApprovalRequested(cb: (request: ApprovalRequest) => void): () => void
  onApprovalResolved(cb: (info: { id: string }) => void): () => void
  onRuntimeUpdate(cb: (update: RuntimeUpdate) => void): () => void
  onChatMessage(cb: (message: ChatMessage) => void): () => void
}
