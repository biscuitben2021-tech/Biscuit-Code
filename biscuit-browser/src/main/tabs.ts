import { BrowserWindow, WebContentsView, session } from 'electron'
import type { AgentView, PageSignature, TabState } from '@shared/types'
import type { ViewBounds } from '@shared/ipc'
import { normalizeUrl } from '@shared/url'
import { runExtract } from './actions/browserActions'
import { buildSignatureScript, type RawSignature } from './agent-view/signature'

const DEFAULT_HOME = 'https://duckduckgo.com/'
// Dedicated, persistent session for browsed pages — fully separate from the
// app UI's session so cookies/storage never mix with the renderer.
const BROWSE_PARTITION = 'persist:biscuit-web'

interface Tab {
  id: string
  view: WebContentsView
  generation: number
  loading: boolean
}

export interface TabManagerCallbacks {
  onTabsChanged: () => void
}

export class TabManager {
  private tabs: Tab[] = []
  private activeId: string | null = null
  private bounds: ViewBounds = { x: 0, y: 88, width: 800, height: 600 }
  private seq = 0

  constructor(
    private readonly win: BrowserWindow,
    private readonly cb: TabManagerCallbacks
  ) {
    win.on('resize', () => this.applyBounds())
  }

  // ── Lifecycle ─────────────────────────────────────────────────────────────
  create(url?: string): string {
    this.seq += 1
    const id = `tab-${this.seq}`
    const view = new WebContentsView({
      webPreferences: {
        // Browsed pages are fully locked down: no node, isolated context, no
        // preload. They can never reach app APIs or the API key.
        contextIsolation: true,
        nodeIntegration: false,
        sandbox: true,
        session: session.fromPartition(BROWSE_PARTITION)
      }
    })
    const tab: Tab = { id, view, generation: 0, loading: true }
    this.tabs.push(tab)
    this.win.contentView.addChildView(view)
    this.wireEvents(tab)
    this.activate(id)
    // Mark loading synchronously (did-start-loading fires a tick later) so the
    // runtime's waitIdle blocks until the new page actually settles.
    void view.webContents.loadURL(url && url.trim() ? normalizeUrl(url) : DEFAULT_HOME)
    return id
  }

  close(id: string): void {
    const idx = this.tabs.findIndex((t) => t.id === id)
    if (idx === -1) return
    const [tab] = this.tabs.splice(idx, 1)
    try {
      this.win.contentView.removeChildView(tab.view)
      tab.view.webContents.close()
    } catch {
      /* already gone */
    }
    if (this.activeId === id) {
      const next = this.tabs[idx] ?? this.tabs[idx - 1] ?? null
      this.activeId = next ? next.id : null
      if (next) this.showOnly(next.id)
    }
    if (this.tabs.length === 0) this.create()
    this.cb.onTabsChanged()
  }

  activate(id: string): void {
    if (!this.tabs.some((t) => t.id === id)) return
    this.activeId = id
    this.showOnly(id)
    this.applyBounds()
    this.cb.onTabsChanged()
  }

  navigate(id: string, url: string): void {
    const tab = this.get(id)
    if (!tab) return
    // Set loading eagerly: loadURL resolves before did-start-loading fires, and
    // the runtime's waitIdle reads this flag to avoid extracting the old page.
    tab.loading = true
    this.cb.onTabsChanged()
    void tab.view.webContents.loadURL(normalizeUrl(url))
  }

  back(id: string): void {
    const wc = this.get(id)?.view.webContents
    if (wc?.canGoBack()) wc.goBack()
  }

  forward(id: string): void {
    const wc = this.get(id)?.view.webContents
    if (wc?.canGoForward()) wc.goForward()
  }

  reload(id: string): void {
    this.get(id)?.view.webContents.reload()
  }

  // ── Bounds (native view positioned under the React layout) ──────────────────
  setBounds(bounds: ViewBounds): void {
    this.bounds = bounds
    this.applyBounds()
  }

  private applyBounds(): void {
    const tab = this.active()
    if (!tab) return
    const b = this.bounds
    tab.view.setBounds({
      x: Math.round(b.x),
      y: Math.round(b.y),
      width: Math.max(0, Math.round(b.width)),
      height: Math.max(0, Math.round(b.height))
    })
  }

  private showOnly(id: string): void {
    for (const tab of this.tabs) tab.view.setVisible(tab.id === id)
  }

  // ── Queries ─────────────────────────────────────────────────────────────────
  active(): Tab | null {
    return this.activeId ? this.get(this.activeId) : null
  }

  get(id: string): Tab | null {
    return this.tabs.find((t) => t.id === id) ?? null
  }

  resolve(id?: string): Tab | null {
    return id ? this.get(id) : this.active()
  }

  list(): TabState[] {
    return this.tabs.map((t) => {
      const wc = t.view.webContents
      return {
        id: t.id,
        title: wc.getTitle() || 'New Tab',
        url: wc.getURL(),
        canGoBack: wc.canGoBack(),
        canGoForward: wc.canGoForward(),
        isLoading: t.loading,
        active: t.id === this.activeId
      }
    })
  }

  /** Compact per-tab summaries for the agent runtime's curated state. */
  summaries(): string {
    return this.list()
      .map((t) => `${t.active ? '*' : ' '}${t.id}: ${t.title} <${t.url}>`)
      .join('\n')
  }

  // ── Agent View + generation/refs ──────────────────────────────────────────
  generationOf(id?: string): number {
    return this.resolve(id)?.generation ?? 0
  }

  async getAgentView(id?: string): Promise<AgentView> {
    const tab = this.resolve(id)
    if (!tab) throw new Error('no active tab')
    // Every extraction re-tags elements (e1..eN), so bump the generation first:
    // each snapshot gets a unique generation and any @ref from a prior snapshot
    // fails the clickRef/typeRef guard instead of aliasing a different element.
    tab.generation += 1
    const raw = await runExtract(tab.view.webContents, tab.generation)
    return {
      tabId: tab.id,
      url: raw.url,
      title: raw.title,
      generation: tab.generation,
      capturedAt: Date.now(),
      headings: raw.headings,
      elements: raw.elements as AgentView['elements'],
      text: raw.text,
      truncated: raw.truncated,
      context: raw.context
    }
  }

  /** Alias of getAgentView — re-extracts and expires prior @refs. */
  refreshAgentView(id?: string): Promise<AgentView> {
    return this.getAgentView(id)
  }

  /**
   * Capture a lightweight page fingerprint for the verification layer. Unlike
   * getAgentView this does NOT re-tag elements or bump the generation, so it is
   * safe to call before/after an action without invalidating live @refs.
   */
  async getSignature(id?: string): Promise<PageSignature> {
    const tab = this.resolve(id)
    if (!tab) throw new Error('no active tab')
    const raw = (await tab.view.webContents.executeJavaScript(buildSignatureScript(), true)) as RawSignature
    return { ...raw, capturedAt: Date.now() }
  }

  // ── Internal ──────────────────────────────────────────────────────────────
  private wireEvents(tab: Tab): void {
    const wc = tab.view.webContents
    const invalidate = (): void => {
      // A new document means old @refs are gone — bump the generation so any
      // ref the agent still holds fails its guard until it re-extracts.
      tab.generation += 1
    }
    wc.on('did-start-loading', () => {
      tab.loading = true
      this.cb.onTabsChanged()
    })
    wc.on('did-stop-loading', () => {
      tab.loading = false
      this.cb.onTabsChanged()
    })
    wc.on('page-title-updated', () => this.cb.onTabsChanged())
    wc.on('did-navigate', () => {
      invalidate()
      this.cb.onTabsChanged()
    })
    wc.on('did-navigate-in-page', () => {
      invalidate()
      this.cb.onTabsChanged()
    })
    // Open target=_blank / window.open in a new tab instead of a popup window.
    wc.setWindowOpenHandler(({ url }) => {
      this.create(url)
      return { action: 'deny' }
    })
  }

  destroy(): void {
    for (const tab of this.tabs) {
      try {
        tab.view.webContents.close()
      } catch {
        /* ignore */
      }
    }
    this.tabs = []
  }
}
