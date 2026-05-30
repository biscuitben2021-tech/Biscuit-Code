import { contextBridge, ipcRenderer, type IpcRendererEvent } from 'electron'
import { IPC, type ViewBounds } from '@shared/ipc'
import type { BiscuitApi } from '@shared/api'
import type {
  ActionProposal,
  PermissionMode,
  SettingsUpdate,
  TaskContract
} from '@shared/types'

// Subscribe to a main->renderer event; returns an unsubscribe fn. The handler
// is wrapped so the renderer never receives the raw IpcRendererEvent.
function on<T>(channel: string, cb: (payload: T) => void): () => void {
  const listener = (_e: IpcRendererEvent, payload: T): void => cb(payload)
  ipcRenderer.on(channel, listener)
  return () => ipcRenderer.removeListener(channel, listener)
}

const api: BiscuitApi = {
  tabs: {
    create: (url) => ipcRenderer.invoke(IPC.TAB_CREATE, url),
    close: (id) => ipcRenderer.invoke(IPC.TAB_CLOSE, id),
    activate: (id) => ipcRenderer.invoke(IPC.TAB_ACTIVATE, id),
    navigate: (id, url) => ipcRenderer.invoke(IPC.TAB_NAVIGATE, id, url),
    back: (id) => ipcRenderer.invoke(IPC.TAB_BACK, id),
    forward: (id) => ipcRenderer.invoke(IPC.TAB_FORWARD, id),
    reload: (id) => ipcRenderer.invoke(IPC.TAB_RELOAD, id),
    list: () => ipcRenderer.invoke(IPC.TAB_LIST)
  },
  view: {
    setBounds: (bounds: ViewBounds) => ipcRenderer.invoke(IPC.VIEW_SET_BOUNDS, bounds)
  },
  agentView: {
    get: (tabId) => ipcRenderer.invoke(IPC.AGENT_VIEW_GET, tabId),
    refresh: (tabId) => ipcRenderer.invoke(IPC.AGENT_VIEW_REFRESH, tabId)
  },
  action: {
    run: (proposal: ActionProposal) => ipcRenderer.invoke(IPC.ACTION_RUN, proposal)
  },
  contract: {
    generate: (prompt) => ipcRenderer.invoke(IPC.CONTRACT_GENERATE, prompt),
    get: () => ipcRenderer.invoke(IPC.CONTRACT_GET),
    lock: (contract?: TaskContract) => ipcRenderer.invoke(IPC.CONTRACT_LOCK, contract),
    clear: () => ipcRenderer.invoke(IPC.CONTRACT_CLEAR)
  },
  mode: {
    get: () => ipcRenderer.invoke(IPC.MODE_GET),
    set: (mode: PermissionMode) => ipcRenderer.invoke(IPC.MODE_SET, mode)
  },
  log: {
    list: () => ipcRenderer.invoke(IPC.LOG_LIST)
  },
  approvals: {
    respond: (id, approved) => ipcRenderer.invoke(IPC.APPROVAL_RESPOND, id, approved)
  },
  settings: {
    get: () => ipcRenderer.invoke(IPC.SETTINGS_GET),
    save: (update: SettingsUpdate) => ipcRenderer.invoke(IPC.SETTINGS_SAVE, update)
  },
  runtime: {
    stop: () => ipcRenderer.invoke(IPC.RUNTIME_STOP)
  },
  chat: {
    send: (message) => ipcRenderer.invoke(IPC.CHAT_SEND, message)
  },

  onTabsChanged: (cb) => on(IPC.EVT_TABS_CHANGED, cb),
  onContractChanged: (cb) => on(IPC.EVT_CONTRACT_CHANGED, cb),
  onModeChanged: (cb) => on(IPC.EVT_MODE_CHANGED, cb),
  onLogAppended: (cb) => on(IPC.EVT_LOG_APPENDED, cb),
  onApprovalRequested: (cb) => on(IPC.EVT_APPROVAL_REQUESTED, cb),
  onApprovalResolved: (cb) => on(IPC.EVT_APPROVAL_RESOLVED, cb),
  onRuntimeUpdate: (cb) => on(IPC.EVT_RUNTIME_UPDATE, cb),
  onChatMessage: (cb) => on(IPC.EVT_CHAT_MESSAGE, cb)
}

// contextIsolation is ON, so this is the only bridge between page and main.
contextBridge.exposeInMainWorld('biscuit', api)
