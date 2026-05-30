import type { LogEntry } from '@shared/types'

let counter = 0
function nextId(): string {
  counter += 1
  return `log-${Date.now()}-${counter}`
}

/**
 * In-memory action/decision log. Every gate decision and executed action is
 * recorded here. Even Bypass mode logs (only the prompts are skipped, not the
 * audit trail). Persisted history is a later-phase TODO.
 */
export class ActionLog {
  private entries: LogEntry[] = []

  constructor(private readonly onAppended: (entry: LogEntry) => void) {}

  add(partial: Omit<LogEntry, 'id' | 'ts'>): LogEntry {
    const entry: LogEntry = { id: nextId(), ts: Date.now(), ...partial }
    this.entries.push(entry)
    // Cap retained entries to keep memory bounded for long sessions.
    if (this.entries.length > 2000) this.entries.splice(0, this.entries.length - 2000)
    this.onAppended(entry)
    return entry
  }

  list(): LogEntry[] {
    return this.entries
  }
}
