// Cross-process types shared by main, preload, and renderer.
// Keep this file free of any runtime imports so it is safe everywhere.

// ── Permission modes ────────────────────────────────────────────────────────
export type PermissionMode = 'safe' | 'assisted' | 'auto' | 'bypass'

export const PERMISSION_MODES: PermissionMode[] = ['safe', 'assisted', 'auto', 'bypass']

export function describeMode(mode: PermissionMode): string {
  switch (mode) {
    case 'safe':
      return 'Review contract, ask before most actions'
    case 'assisted':
      return 'Auto low-risk actions, ask before sensitive ones'
    case 'auto':
      return 'Act inside the contract, ask only for high-risk'
    case 'bypass':
      return 'Expert: no prompts, no contract lock (logs + stop stay on)'
  }
}

// ── Agent View ──────────────────────────────────────────────────────────────
export interface BoundingBox {
  x: number
  y: number
  width: number
  height: number
}

export interface ElementState {
  visible: boolean
  enabled: boolean
  checked?: boolean
  focused?: boolean
  inViewport: boolean
}

export type ElementRole =
  | 'link'
  | 'button'
  | 'textbox'
  | 'checkbox'
  | 'radio'
  | 'select'
  | 'submit'
  | 'other'

export interface AgentElement {
  ref: string // stable short ref e.g. "@e1"
  role: ElementRole
  tag: string
  name: string // accessible label / visible text
  href?: string
  type?: string // input type / button type
  value?: string
  state: ElementState
  box: BoundingBox
  /** True for fields the gate treats as sensitive (password, payment, email...). */
  sensitive?: boolean
}

export interface AgentHeading {
  level: number
  text: string
}

export interface AgentView {
  tabId: string
  url: string
  title: string
  generation: number // bumped on navigation / refresh; refs expire across generations
  capturedAt: number
  headings: AgentHeading[]
  elements: AgentElement[]
  text: string // compact visible text snapshot
  truncated: boolean
}

// ── Tabs ────────────────────────────────────────────────────────────────────
export interface TabState {
  id: string
  title: string
  url: string
  canGoBack: boolean
  canGoForward: boolean
  isLoading: boolean
  active: boolean
}

// ── Task Contract ───────────────────────────────────────────────────────────
export type ContractActionName =
  | 'open'
  | 'read'
  | 'search'
  | 'scroll'
  | 'click'
  | 'type'
  | 'submit'
  | 'login'
  | 'payment'
  | 'upload'
  | 'download'
  | 'send'
  | 'delete'
  | 'settings'

export interface TaskContract {
  goal: string
  allowed_actions: ContractActionName[]
  requires_user_confirmation: ContractActionName[]
  blocked_without_user_override: ContractActionName[]
}

export type ContractStatus = 'none' | 'draft' | 'locked'

export interface ContractState {
  status: ContractStatus
  contract: TaskContract | null
  /** The original user prompt this contract was generated from (display only). */
  prompt: string
  createdAt: number
  lockedAt: number | null
}

// ── Actions ─────────────────────────────────────────────────────────────────
export type ActionKind =
  | 'openUrl'
  | 'clickRef'
  | 'typeRef'
  | 'scroll'
  | 'refreshAgentView'
  | 'screenshot'
  | 'done'
  | 'ask'

export type ScrollDirection = 'up' | 'down' | 'top' | 'bottom'

export interface ActionProposal {
  kind: ActionKind
  url?: string
  ref?: string
  text?: string
  direction?: ScrollDirection
  pages?: number
  /** Model-supplied reasoning for transparency (never executed as code). */
  rationale?: string
  /** For kind === 'ask' | 'done': the message to surface to the user. */
  message?: string
}

export interface ActionResult {
  ok: boolean
  detail: string
  /** Optional structured payload (e.g. screenshot data URL). */
  data?: unknown
}

export type ActionRisk = 'low' | 'medium' | 'high'

// ── Action Gate ─────────────────────────────────────────────────────────────
export type GateDecision = 'allow' | 'ask' | 'block'

export interface GateResult {
  decision: GateDecision
  risk: ActionRisk
  reason: string
}

// ── Logging ─────────────────────────────────────────────────────────────────
export type LogType = 'gate' | 'action' | 'runtime' | 'contract' | 'system'

export interface LogEntry {
  id: string
  ts: number
  type: LogType
  mode?: PermissionMode
  action?: ActionKind
  decision?: GateDecision
  risk?: ActionRisk
  message: string
}

// ── Approvals ───────────────────────────────────────────────────────────────
export interface ApprovalRequest {
  id: string
  proposal: ActionProposal
  gate: GateResult
}

// ── Settings ────────────────────────────────────────────────────────────────
export type LlmProvider = 'openai' | 'anthropic' | 'google' | 'openai_compatible' | 'lmstudio'

export interface Settings {
  provider: LlmProvider
  model: string
  baseUrl: string
  defaultMode: PermissionMode
  /** Never contains the raw key — only whether one is stored. */
  hasApiKey: boolean
}

/** Payload used when the renderer saves settings (may include a new key). */
export interface SettingsUpdate {
  provider: LlmProvider
  model: string
  baseUrl: string
  defaultMode: PermissionMode
  apiKey?: string // optional; omitted means "keep existing"
}

// ── Runtime ─────────────────────────────────────────────────────────────────
export type RuntimeStatus = 'running' | 'awaiting-approval' | 'done' | 'error' | 'stopped'

export interface RuntimeUpdate {
  status: RuntimeStatus
  message: string
}

// ── Chat ────────────────────────────────────────────────────────────────────
export interface ChatMessage {
  id: string
  role: 'user' | 'assistant' | 'system'
  text: string
  ts: number
}
