import { useEffect, useState } from 'react'
import { useBiscuit } from './state/store'
import { TabBar } from './components/TabBar'
import { Toolbar } from './components/Toolbar'
import { BrowserViewport } from './components/BrowserViewport'
import { SidePanel, type PanelKey } from './components/SidePanel'
import { BypassBanner } from './components/BypassBanner'
import { BypassConfirmModal } from './components/BypassConfirmModal'
import { Onboarding } from './components/Onboarding'

const ONBOARDED_KEY = 'biscuit-onboarded-v1'

export function App(): JSX.Element {
  const state = useBiscuit()
  const [panel, setPanel] = useState<PanelKey>('chat')
  const [bypassModal, setBypassModal] = useState(false)
  const [notice, setNotice] = useState('')
  const [onboard, setOnboard] = useState(false)

  useEffect(() => {
    try {
      if (!localStorage.getItem(ONBOARDED_KEY)) setOnboard(true)
    } catch {
      /* localStorage unavailable — skip onboarding */
    }
  }, [])

  const activeTab = state.tabs.find((t) => t.id === state.activeTabId) ?? null
  // Surface attention on tabs that need the user.
  const badge = state.approvals.length > 0 ? state.approvals.length : undefined
  const expertMode = state.settings?.expertMode ?? false
  const bypassArmed = state.mode === 'bypass'
  const running = state.runtime?.status === 'running' || state.runtime?.status === 'awaiting-approval'
  // The native browser view paints over the DOM, so collapse it while a modal
  // is up or it would occlude the dialog.
  const modalOpen = bypassModal || onboard

  const armBypass = (): void => {
    if (!expertMode) {
      setNotice('Bypass requires Expert mode — enable it in Settings first.')
      window.setTimeout(() => setNotice(''), 4000)
      setPanel('settings')
      return
    }
    setBypassModal(true)
  }

  const confirmBypass = (): void => {
    setBypassModal(false)
    void window.biscuit.mode.set('bypass')
  }

  const finishOnboarding = (openSettings: boolean): void => {
    try {
      localStorage.setItem(ONBOARDED_KEY, '1')
    } catch {
      /* ignore */
    }
    setOnboard(false)
    if (openSettings) setPanel('settings')
  }

  return (
    <div className={`app ${bypassArmed ? 'app-bypass' : ''}`}>
      {bypassArmed && <BypassBanner running={running} />}
      {notice && <div className="notice-bar">{notice}</div>}
      <div className="app-body">
        <div className="browser">
          <TabBar tabs={state.tabs} />
          <Toolbar
            activeTab={activeTab}
            mode={state.mode}
            runtime={state.runtime}
            expertMode={expertMode}
            onArmBypass={armBypass}
          />
          <BrowserViewport hasTab={!!activeTab} hidden={modalOpen} />
        </div>
        <SidePanel active={panel} onSelect={setPanel} approvalBadge={badge} />
      </div>
      <BypassConfirmModal
        open={bypassModal}
        onCancel={() => setBypassModal(false)}
        onConfirm={confirmBypass}
      />
      {onboard && <Onboarding onClose={finishOnboarding} />}
    </div>
  )
}
