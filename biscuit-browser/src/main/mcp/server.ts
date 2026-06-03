import http from 'node:http'
import type {
  ActionProposal,
  ActionResult,
  AgentView,
  ContractStatus,
  PermissionMode,
  TabState
} from '@shared/types'

// A tiny, dependency-free MCP server (JSON-RPC 2.0 over HTTP, bound to
// localhost) that exposes Biscuit Browser's capabilities as tools so the
// Biscuits CLI and other AI agents can drive the browser. Reading tools
// (agent view, tabs, screenshot, status) are free; acting tools (open/click/
// type/scroll/new tab) are routed through the Action Gate, so external agents
// are held to the same permission model as the built-in agent.
//
// Transport: Streamable-HTTP style — clients POST JSON-RPC to `/mcp` and get a
// single JSON response. Stateless; no server-initiated messages.

const PROTOCOL_VERSION = '2025-06-18'
const SERVER_INFO = { name: 'biscuit-browser', version: '0.1.0' }

/** The browser capabilities the MCP tools call into (provided by App). */
export interface McpToolDeps {
  /** Fresh structured Agent View of the active tab (and cache it for gating). */
  getAgentView: () => Promise<AgentView>
  /** Run an acting proposal through the Action Gate, then execute. */
  runAction: (proposal: ActionProposal) => Promise<ActionResult>
  /** Fallback screenshot (PNG data URL in result.data). */
  screenshot: () => Promise<ActionResult>
  listTabs: () => TabState[]
  newTab: (url?: string) => string
  status: () => { mode: PermissionMode; contract: ContractStatus; running: boolean }
  log: (message: string) => void
}

export interface McpServerHandle {
  url: string
  port: number
  close: () => Promise<void>
}

type Content = { type: 'text'; text: string } | { type: 'image'; data: string; mimeType: string }

interface Tool {
  name: string
  description: string
  inputSchema: Record<string, unknown>
  handler: (args: Record<string, unknown>) => Promise<Content[]>
}

const ok = (text: string): Content[] => [{ type: 'text', text }]
const json = (data: unknown): Content[] => [{ type: 'text', text: JSON.stringify(data, null, 2) }]

function buildTools(deps: McpToolDeps): Tool[] {
  // Compact the Agent View so an agent gets refs + text without raw HTML.
  const compactView = (v: AgentView): unknown => ({
    url: v.url,
    title: v.title,
    generation: v.generation,
    headings: v.headings,
    elements: v.elements.map((e) => ({
      ref: e.ref,
      role: e.role,
      name: e.name,
      ...(e.type ? { type: e.type } : {}),
      ...(e.href ? { href: e.href } : {}),
      ...(e.sensitive ? { sensitive: true } : {}),
      ...(e.via ? { via: e.via } : {}),
      ...(e.state.inViewport ? {} : { offscreen: true }),
      ...(e.state.enabled ? {} : { disabled: true })
    })),
    text: v.text.slice(0, 4000),
    context: v.context,
    truncated: v.truncated
  })

  return [
    {
      name: 'browser_get_agent_view',
      description:
        'Read the active tab as a structured Agent View: visible text, headings, and interactive elements with stable refs (@e1, @e2…). Call this first, then refer to elements by their @ref in browser_click / browser_type. No raw HTML or screenshots.',
      inputSchema: { type: 'object', properties: {}, additionalProperties: false },
      handler: async () => json(compactView(await deps.getAgentView()))
    },
    {
      name: 'browser_open_url',
      description:
        'Navigate the active tab to a URL (http/https). Gated by the permission mode + task contract.',
      inputSchema: {
        type: 'object',
        properties: { url: { type: 'string', description: 'Absolute http(s) URL or a search query.' } },
        required: ['url'],
        additionalProperties: false
      },
      handler: async (a) => {
        const r = await deps.runAction({ kind: 'openUrl', url: String(a.url ?? '') })
        return ok(r.detail)
      }
    },
    {
      name: 'browser_click',
      description:
        'Click an element by its @ref (from browser_get_agent_view). Gated. Re-read the Agent View afterwards.',
      inputSchema: {
        type: 'object',
        properties: { ref: { type: 'string', description: 'An @e ref, e.g. "@e3".' } },
        required: ['ref'],
        additionalProperties: false
      },
      handler: async (a) => {
        const r = await deps.runAction({ kind: 'clickRef', ref: String(a.ref ?? '') })
        return ok(r.detail)
      }
    },
    {
      name: 'browser_type',
      description:
        'Type text into an input/textarea/contenteditable (or select an option) by its @ref. Gated.',
      inputSchema: {
        type: 'object',
        properties: { ref: { type: 'string' }, text: { type: 'string' } },
        required: ['ref', 'text'],
        additionalProperties: false
      },
      handler: async (a) => {
        const r = await deps.runAction({
          kind: 'typeRef',
          ref: String(a.ref ?? ''),
          text: String(a.text ?? '')
        })
        return ok(r.detail)
      }
    },
    {
      name: 'browser_scroll',
      description: 'Scroll the active tab.',
      inputSchema: {
        type: 'object',
        properties: {
          direction: { type: 'string', enum: ['up', 'down', 'top', 'bottom'] },
          pages: { type: 'number', description: 'How many viewport pages (default 1).' }
        },
        additionalProperties: false
      },
      handler: async (a) => {
        const direction = (['up', 'down', 'top', 'bottom'] as const).includes(a.direction as never)
          ? (a.direction as 'up' | 'down' | 'top' | 'bottom')
          : 'down'
        const pages = typeof a.pages === 'number' ? a.pages : 1
        const r = await deps.runAction({ kind: 'scroll', direction, pages })
        return ok(r.detail)
      }
    },
    {
      name: 'browser_screenshot',
      description: 'Capture a PNG of the visible page (fallback — prefer browser_get_agent_view).',
      inputSchema: { type: 'object', properties: {}, additionalProperties: false },
      handler: async () => {
        const r = await deps.screenshot()
        if (r.ok && typeof r.data === 'string' && r.data.startsWith('data:image')) {
          const base64 = r.data.slice(r.data.indexOf(',') + 1)
          return [{ type: 'image', data: base64, mimeType: 'image/png' }]
        }
        return ok(r.detail)
      }
    },
    {
      name: 'browser_list_tabs',
      description: 'List open tabs (id, title, url, active).',
      inputSchema: { type: 'object', properties: {}, additionalProperties: false },
      handler: async () => json(deps.listTabs())
    },
    {
      name: 'browser_new_tab',
      description: 'Open a new tab (optionally at a URL) and make it active.',
      inputSchema: {
        type: 'object',
        properties: { url: { type: 'string' } },
        additionalProperties: false
      },
      handler: async (a) => {
        const id = deps.newTab(a.url ? String(a.url) : undefined)
        return ok(`opened tab ${id}`)
      }
    },
    {
      name: 'browser_status',
      description:
        'Current permission mode, task-contract status, and whether the built-in agent is running.',
      inputSchema: { type: 'object', properties: {}, additionalProperties: false },
      handler: async () => json(deps.status())
    }
  ]
}

// ── JSON-RPC dispatch ─────────────────────────────────────────────────────────
interface RpcRequest {
  jsonrpc: '2.0'
  id?: string | number | null
  method: string
  params?: Record<string, unknown>
}

function rpcError(id: string | number | null | undefined, code: number, message: string): object {
  return { jsonrpc: '2.0', id: id ?? null, error: { code, message } }
}
function rpcResult(id: string | number | null | undefined, result: unknown): object {
  return { jsonrpc: '2.0', id: id ?? null, result }
}

async function dispatch(msg: RpcRequest, tools: Tool[], deps: McpToolDeps): Promise<object | null> {
  const isNotification = msg.id === undefined || msg.id === null
  const method = msg.method

  // Notifications (no id) get no response.
  if (isNotification) {
    return null
  }

  switch (method) {
    case 'initialize':
      return rpcResult(msg.id, {
        protocolVersion:
          typeof msg.params?.protocolVersion === 'string'
            ? (msg.params.protocolVersion as string)
            : PROTOCOL_VERSION,
        capabilities: { tools: { listChanged: false } },
        serverInfo: SERVER_INFO
      })
    case 'ping':
      return rpcResult(msg.id, {})
    case 'tools/list':
      return rpcResult(msg.id, {
        tools: tools.map((t) => ({ name: t.name, description: t.description, inputSchema: t.inputSchema }))
      })
    case 'tools/call': {
      const name = String(msg.params?.name ?? '')
      const args = (msg.params?.arguments as Record<string, unknown>) ?? {}
      const tool = tools.find((t) => t.name === name)
      if (!tool) return rpcResult(msg.id, { content: ok(`Unknown tool: ${name}`), isError: true })
      try {
        deps.log(`mcp tool call: ${name}`)
        const content = await tool.handler(args)
        return rpcResult(msg.id, { content })
      } catch (err) {
        // Tool errors are reported in the result (isError), not as JSON-RPC errors.
        return rpcResult(msg.id, {
          content: ok(`Tool "${name}" failed: ${(err as Error).message}`),
          isError: true
        })
      }
    }
    default:
      return rpcError(msg.id, -32601, `Method not found: ${method}`)
  }
}

function readBody(req: http.IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    let data = ''
    let size = 0
    req.on('data', (chunk) => {
      size += chunk.length
      if (size > 5_000_000) {
        reject(new Error('request body too large'))
        req.destroy()
        return
      }
      data += chunk
    })
    req.on('end', () => resolve(data))
    req.on('error', reject)
  })
}

async function handle(
  req: http.IncomingMessage,
  res: http.ServerResponse,
  tools: Tool[],
  deps: McpToolDeps
): Promise<void> {
  const send = (status: number, body?: object): void => {
    res.writeHead(status, { 'Content-Type': 'application/json' })
    res.end(body === undefined ? undefined : JSON.stringify(body))
  }

  const url = (req.url ?? '/').split('?')[0]
  if (url !== '/mcp' && url !== '/') {
    send(404, rpcError(null, -32600, 'Not found'))
    return
  }
  if (req.method === 'GET') {
    // Health/info probe (we don't open a server→client SSE stream).
    send(200, {
      name: SERVER_INFO.name,
      version: SERVER_INFO.version,
      transport: 'http-json',
      endpoint: '/mcp'
    })
    return
  }
  if (req.method !== 'POST') {
    send(405, rpcError(null, -32600, 'Method not allowed'))
    return
  }

  let parsed: unknown
  try {
    parsed = JSON.parse(await readBody(req))
  } catch {
    send(400, rpcError(null, -32700, 'Parse error'))
    return
  }

  // Single request or JSON-RPC batch.
  if (Array.isArray(parsed)) {
    const responses = (await Promise.all(parsed.map((m) => dispatch(m as RpcRequest, tools, deps)))).filter(
      Boolean
    )
    if (responses.length === 0) {
      send(202)
      return
    }
    send(200, responses as unknown as object)
    return
  }

  const response = await dispatch(parsed as RpcRequest, tools, deps)
  if (response === null) {
    send(202)
    return
  }
  send(200, response)
}

function listen(server: http.Server, startPort: number): Promise<number> {
  return new Promise((resolve, reject) => {
    let port = startPort
    let attempts = 0
    const tryListen = (): void => {
      server.once('error', (err: NodeJS.ErrnoException) => {
        if (err.code === 'EADDRINUSE' && attempts < 15) {
          attempts += 1
          port += 1
          tryListen()
        } else {
          reject(err)
        }
      })
      server.listen(port, '127.0.0.1', () => resolve(port))
    }
    tryListen()
  })
}

/** Start the MCP server on the first free port at/after `preferredPort`. */
export async function startMcpServer(deps: McpToolDeps, preferredPort = 8765): Promise<McpServerHandle> {
  const tools = buildTools(deps)
  const server = http.createServer((req, res) => {
    handle(req, res, tools, deps).catch(() => {
      try {
        res.writeHead(500, { 'Content-Type': 'application/json' })
        res.end(JSON.stringify(rpcError(null, -32603, 'Internal error')))
      } catch {
        /* response already sent */
      }
    })
  })
  const port = await listen(server, preferredPort)
  return {
    url: `http://127.0.0.1:${port}/mcp`,
    port,
    close: () => new Promise<void>((resolve) => server.close(() => resolve()))
  }
}
