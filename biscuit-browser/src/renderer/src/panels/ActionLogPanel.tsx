import { useBiscuit } from '../state/store'

function time(ts: number): string {
  const d = new Date(ts)
  return d.toLocaleTimeString()
}

export function ActionLogPanel(): JSX.Element {
  const { logs } = useBiscuit()
  return (
    <div className="panel-scroll">
      {logs.length === 0 && (
        <p className="muted">No activity yet. Every gate decision and action is logged here.</p>
      )}
      {logs
        .slice()
        .reverse()
        .map((e) => (
          <div key={e.id} className="log-entry">
            <span className="ts">{time(e.ts)}</span> <b>{e.type}</b>
            {e.action ? ` ${e.action}` : ''}
            {e.decision ? (
              <span className={`dec-${e.decision}`}>
                {' '}
                {e.decision}
                {e.risk ? `/${e.risk}` : ''}
              </span>
            ) : (
              ''
            )}
            {e.mode ? <span className="muted"> [{e.mode}]</span> : ''}
            {'\n'}
            {e.message}
          </div>
        ))}
    </div>
  )
}
