// Central registry of IPC channel names + the typed shape of the preload API.
// Channels are grouped: `invoke` (renderer -> main, request/response) and
// `event` (main -> renderer, push). Keeping names here avoids string drift.

export const IPC = {
  // Tabs
  TAB_CREATE: 'tab:create',
  TAB_CLOSE: 'tab:close',
  TAB_ACTIVATE: 'tab:activate',
  TAB_NAVIGATE: 'tab:navigate',
  TAB_BACK: 'tab:back',
  TAB_FORWARD: 'tab:forward',
  TAB_RELOAD: 'tab:reload',
  TAB_LIST: 'tab:list',

  // Native browser view bounds (renderer reports where the page should render)
  VIEW_SET_BOUNDS: 'view:setBounds',

  // Agent View + actions
  AGENT_VIEW_GET: 'agent:getView',
  AGENT_VIEW_REFRESH: 'agent:refreshView',
  ACTION_RUN: 'action:run', // inspection-only manual action from the UI (screenshot / refreshAgentView)

  // Task Contract
  CONTRACT_GENERATE: 'contract:generate',
  CONTRACT_GET: 'contract:get',
  CONTRACT_LOCK: 'contract:lock',
  CONTRACT_CLEAR: 'contract:clear',

  // Permission mode
  MODE_GET: 'mode:get',
  MODE_SET: 'mode:set',

  // Logs
  LOG_LIST: 'log:list',

  // Approvals
  APPROVAL_RESPOND: 'approval:respond',

  // Settings
  SETTINGS_GET: 'settings:get',
  SETTINGS_SAVE: 'settings:save',

  // Runtime (agent executor)
  RUNTIME_STOP: 'runtime:stop', // also the emergency stop

  // Chat
  CHAT_SEND: 'chat:send',

  // Demo mode (no API key): scripted contract + gate decisions
  DEMO_RUN: 'demo:run',

  // MCP server (exposes the browser to external AI agents)
  MCP_GET_INFO: 'mcp:getInfo',

  // ── Events (main -> renderer) ──
  EVT_TABS_CHANGED: 'evt:tabsChanged',
  EVT_CONTRACT_CHANGED: 'evt:contractChanged',
  EVT_MODE_CHANGED: 'evt:modeChanged',
  EVT_LOG_APPENDED: 'evt:logAppended',
  EVT_APPROVAL_REQUESTED: 'evt:approvalRequested',
  EVT_APPROVAL_RESOLVED: 'evt:approvalResolved',
  EVT_RUNTIME_UPDATE: 'evt:runtimeUpdate',
  EVT_CHAT_MESSAGE: 'evt:chatMessage'
} as const

export interface ViewBounds {
  x: number
  y: number
  width: number
  height: number
}
