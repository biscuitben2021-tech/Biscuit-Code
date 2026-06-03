import { ChatPanel } from '../panels/ChatPanel'
import { ContractPanel } from '../panels/ContractPanel'
import { AgentViewPanel } from '../panels/AgentViewPanel'
import { ActionLogPanel } from '../panels/ActionLogPanel'
import { SettingsPanel } from '../panels/SettingsPanel'
import { StatusBar } from './StatusBar'

// Approvals are handled inline in the Chat (Approve/Deny cards), so there is no
// separate Approvals tab.
export type PanelKey = 'chat' | 'contract' | 'agentView' | 'log' | 'settings'

const TABS: [PanelKey, string][] = [
  ['chat', 'Chat'],
  ['contract', 'Contract'],
  ['agentView', 'View'],
  ['log', 'Log'],
  ['settings', 'Settings']
]

interface Props {
  active: PanelKey
  onSelect: (key: PanelKey) => void
}

export function SidePanel({ active, onSelect }: Props): JSX.Element {
  return (
    <div className="panel">
      <StatusBar />
      <div className="panel-tabs">
        {TABS.map(([key, label]) => (
          <button key={key} className={active === key ? 'active' : ''} onClick={() => onSelect(key)}>
            {label}
          </button>
        ))}
      </div>
      <div className="panel-body">
        {active === 'chat' && <ChatPanel />}
        {active === 'contract' && <ContractPanel />}
        {active === 'agentView' && <AgentViewPanel />}
        {active === 'log' && <ActionLogPanel />}
        {active === 'settings' && <SettingsPanel />}
      </div>
    </div>
  )
}
