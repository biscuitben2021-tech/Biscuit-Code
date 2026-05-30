import type { TabState } from '@shared/types'

export function TabBar({ tabs }: { tabs: TabState[] }): JSX.Element {
  return (
    <div className="tabbar">
      {tabs.map((tab) => (
        <div
          key={tab.id}
          className={`tab ${tab.active ? 'active' : ''}`}
          onClick={() => void window.biscuit.tabs.activate(tab.id)}
          title={tab.url}
        >
          <span className="title">{tab.isLoading ? '… ' : ''}{tab.title || 'New Tab'}</span>
          <button
            className="close"
            title="Close tab"
            onClick={(e) => {
              e.stopPropagation()
              void window.biscuit.tabs.close(tab.id)
            }}
          >
            ×
          </button>
        </div>
      ))}
      <button className="tab-add" title="New tab" onClick={() => void window.biscuit.tabs.create()}>
        +
      </button>
    </div>
  )
}
