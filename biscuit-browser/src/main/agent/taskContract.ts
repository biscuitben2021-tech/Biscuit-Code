import type { ContractActionName, TaskContract } from '@shared/types'
import { CONTRACT_SYSTEM } from './prompts'
import { llmJson, type LlmConfig } from './llm'

const KNOWN_ACTIONS: ContractActionName[] = [
  'open',
  'read',
  'search',
  'scroll',
  'click',
  'type',
  'submit',
  'login',
  'payment',
  'upload',
  'download',
  'send',
  'delete',
  'settings'
]

function normalizeActions(value: unknown): ContractActionName[] {
  if (!Array.isArray(value)) return []
  const out: ContractActionName[] = []
  for (const raw of value) {
    if (typeof raw !== 'string') continue
    const a = raw.trim().toLowerCase() as ContractActionName
    if (KNOWN_ACTIONS.includes(a) && !out.includes(a)) out.push(a)
  }
  return out
}

/**
 * The TaskContractAgent. It sees ONLY the original user prompt — never page
 * text, Agent View, or any browser content. The locked contract is the
 * authority for the Action Gate; browser/page content can never modify it.
 */
export async function generateContract(cfg: LlmConfig, prompt: string): Promise<TaskContract> {
  const clean = prompt.trim()
  try {
    const raw = await llmJson<Record<string, unknown>>(
      cfg,
      CONTRACT_SYSTEM,
      `User request:\n"""\n${clean}\n"""`
    )
    const goal = typeof raw.goal === 'string' && raw.goal.trim() ? raw.goal.trim() : clean
    const contract: TaskContract = {
      goal: goal.slice(0, 400),
      allowed_actions: normalizeActions(raw.allowed_actions),
      requires_user_confirmation: normalizeActions(raw.requires_user_confirmation),
      blocked_without_user_override: normalizeActions(raw.blocked_without_user_override)
    }
    // If the model returned nothing usable, fall back to the safe default.
    if (contract.allowed_actions.length === 0) return fallbackContract(clean)
    return contract
  } catch (err) {
    console.error('[contract] generation failed, using safe default:', err)
    return fallbackContract(clean)
  }
}

/** Conservative research-oriented default used when no model is configured. */
function fallbackContract(prompt: string): TaskContract {
  return {
    goal: prompt.trim().slice(0, 300) || 'Assist with the requested browser task',
    allowed_actions: ['open', 'read', 'search', 'scroll', 'click'],
    requires_user_confirmation: ['type', 'submit', 'upload', 'download'],
    blocked_without_user_override: ['login', 'payment', 'send', 'delete', 'settings']
  }
}
