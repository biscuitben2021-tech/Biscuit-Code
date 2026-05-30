import type { ActionProposal, ActionResult } from '@shared/types'
import type { TabManager } from '../tabs'
import { clickRef, scroll, screenshot, typeRef } from './browserActions'

/**
 * Execute an already-gated action proposal against the active tab. Used by both
 * the agent runtime and manual UI actions. Control-flow kinds (done/ask) are
 * handled by the caller, not here.
 */
export async function executeProposal(tabs: TabManager, proposal: ActionProposal): Promise<ActionResult> {
  const tab = tabs.active()
  if (!tab && proposal.kind !== 'openUrl') {
    return { ok: false, detail: 'no active tab' }
  }
  const generation = tabs.generationOf()

  switch (proposal.kind) {
    case 'openUrl': {
      if (!proposal.url) return { ok: false, detail: 'openUrl requires a url' }
      if (tab) tabs.navigate(tab.id, proposal.url)
      else tabs.create(proposal.url)
      return { ok: true, detail: `opened ${proposal.url}` }
    }
    case 'clickRef': {
      if (!proposal.ref) return { ok: false, detail: 'clickRef requires a ref' }
      return clickRef(tab!.view.webContents, proposal.ref, generation)
    }
    case 'typeRef': {
      if (!proposal.ref) return { ok: false, detail: 'typeRef requires a ref' }
      if (typeof proposal.text !== 'string') return { ok: false, detail: 'typeRef requires text' }
      return typeRef(tab!.view.webContents, proposal.ref, proposal.text, generation)
    }
    case 'scroll':
      return scroll(tab!.view.webContents, proposal.direction ?? 'down', proposal.pages ?? 1)
    case 'refreshAgentView': {
      const view = await tabs.refreshAgentView()
      return { ok: true, detail: `refreshed Agent View (${view.elements.length} elements)` }
    }
    case 'screenshot':
      return screenshot(tab!.view.webContents)
    case 'done':
    case 'ask':
      return { ok: true, detail: proposal.message ?? proposal.kind }
  }
}
