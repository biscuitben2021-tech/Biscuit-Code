import { ChatPanel } from '../panels/ChatPanel'
import { ContractPanel } from '../panels/ContractPanel'
import { AgentViewPanel } from '../panels/AgentViewPanel'
import { ActionLogPanel } from '../panels/ActionLogPanel'
import { ApprovalsPanel } from '../panels/ApprovalsPanel'
import { SettingsPanel } from '../panels/SettingsPanel'
import { StatusBar } from './StatusBar'

export type PanelKey = 'chat' | 'contract' | 'agentView' | 'log' | 'approvals' | 'settings'

const TABS: [PanelKey, string][] = [
  ['chat', 'Chat'],
  ['contract', 'Contract'],
  ['agentView', 'Agent View'],
  ['log', 'Action Log'],
  ['approvals', 'Approvals'],
  ['settings', 'Settings']
]

interface Props {
  active: PanelKey
  onSelect: (key: PanelKey) => void
  approvalBadge?: number
}

export function SidePanel({ active, onSelect, approvalBadge }: Props): JSX.Element {
  return (
    <div className="panel">
      <StatusBar />
      <div className="panel-tabs">
        {TABS.map(([key, label]) => (
          <button key={key} className={active === key ? 'active' : ''} onClick={() => onSelect(key)}>
            {label}
            {key === 'approvals' && approvalBadge ? ` (${approvalBadge})` : ''}
          </button>
        ))}
      </div>
      <div className="panel-body">
        {active === 'chat' && <ChatPanel />}
        {active === 'contract' && <ContractPanel />}
        {active === 'agentView' && <AgentViewPanel />}
        {active === 'log' && <ActionLogPanel />}
        {active === 'approvals' && <ApprovalsPanel />}
        {active === 'settings' && <SettingsPanel />}
      </div>
    </div>
  )
}
