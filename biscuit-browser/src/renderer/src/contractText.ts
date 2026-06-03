import type { TaskContract } from '@shared/types'

const list = (xs: string[]): string => (xs.length ? xs.join(', ') : 'none')

/**
 * Plain-text, to-do-style rendering of a Task Contract — what the user reads
 * (in the Contract panel and in the chat). Deliberately NOT JSON.
 */
export function contractToTodo(c: TaskContract): string {
  return [
    `Goal: ${c.goal}`,
    '',
    `✅ Allowed: ${list(c.allowed_actions)}`,
    `⚠️ Ask me first: ${list(c.requires_user_confirmation)}`,
    `⛔ Needs override: ${list(c.blocked_without_user_override)}`
  ].join('\n')
}
