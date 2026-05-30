import { useState } from 'react'
import { useBiscuit } from './state/store'
import { TabBar } from './components/TabBar'
import { Toolbar } from './components/Toolbar'
import { BrowserViewport } from './components/BrowserViewport'
import { SidePanel, type PanelKey } from './components/SidePanel'

export function App(): JSX.Element {
  const state = useBiscuit()
  const [panel, setPanel] = useState<PanelKey>('chat')
  const activeTab = state.tabs.find((t) => t.id === state.activeTabId) ?? null
  // Surface attention on tabs that need the user.
  const badge = state.approvals.length > 0 ? state.approvals.length : undefined

  return (
    <div className="app">
      <div className="browser">
        <TabBar tabs={state.tabs} />
        <Toolbar activeTab={activeTab} mode={state.mode} runtime={state.runtime} />
        <BrowserViewport hasTab={!!activeTab} />
      </div>
      <SidePanel active={panel} onSelect={setPanel} approvalBadge={badge} />
    </div>
  )
}
