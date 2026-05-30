import type { LlmProvider } from '@shared/types'

// Provider-agnostic LLM client. Deliberately mirrors the provider set of the
// Rust "biscuits" CLI (OpenAI / Anthropic / Google / OpenAI-compatible /
// LM Studio) so the two can later share a config — or so this client can be
// swapped for a call into the Rust binary. See README "Connecting the Rust CLI".

export interface LlmConfig {
  provider: LlmProvider
  model: string
  baseUrl: string
  apiKey: string | null
}

export class LlmError extends Error {}

function cleanBase(url: string): string {
  return url.replace(/\/+$/, '')
}

/** One non-streaming completion. Returns the assistant text. */
async function llmComplete(cfg: LlmConfig, system: string, user: string): Promise<string> {
  if (!cfg.model) throw new LlmError('No model configured. Open Settings.')
  switch (cfg.provider) {
    case 'openai':
    case 'openai_compatible':
    case 'lmstudio':
      return openaiComplete(cfg, system, user)
    case 'anthropic':
      return anthropicComplete(cfg, system, user)
    case 'google':
      return googleComplete(cfg, system, user)
  }
}

/** Completion + best-effort JSON parse. Throws LlmError on parse failure. */
export async function llmJson<T>(cfg: LlmConfig, system: string, user: string): Promise<T> {
  const raw = await llmComplete(cfg, system, user)
  const parsed = extractJson(raw)
  if (parsed === null) throw new LlmError(`Model did not return JSON:\n${raw.slice(0, 400)}`)
  return parsed as T
}

/** Find the first balanced JSON object/array in arbitrary model text. */
export function extractJson(text: string): unknown {
  const trimmed = text.trim()
  try {
    return JSON.parse(trimmed)
  } catch {
    /* fall through */
  }
  // Strip ```json fences if present.
  const fence = trimmed.match(/```(?:json)?\s*([\s\S]*?)```/)
  if (fence) {
    try {
      return JSON.parse(fence[1].trim())
    } catch {
      /* fall through */
    }
  }
  const start = trimmed.search(/[{[]/)
  if (start === -1) return null
  const open = trimmed[start]
  const close = open === '{' ? '}' : ']'
  let depth = 0
  let inStr = false
  let esc = false
  for (let i = start; i < trimmed.length; i++) {
    const ch = trimmed[i]
    if (inStr) {
      if (esc) esc = false
      else if (ch === '\\') esc = true
      else if (ch === '"') inStr = false
      continue
    }
    if (ch === '"') inStr = true
    else if (ch === open) depth++
    else if (ch === close) {
      depth--
      if (depth === 0) {
        try {
          return JSON.parse(trimmed.slice(start, i + 1))
        } catch {
          return null
        }
      }
    }
  }
  return null
}

async function postJson(url: string, headers: Record<string, string>, body: unknown): Promise<any> {
  let res: Response
  try {
    res = await fetch(url, { method: 'POST', headers, body: JSON.stringify(body) })
  } catch (err) {
    throw new LlmError(`Network error contacting model provider: ${(err as Error).message}`)
  }
  if (!res.ok) {
    const text = await res.text().catch(() => '')
    throw new LlmError(`Provider request failed (${res.status}): ${text.slice(0, 400)}`)
  }
  return res.json()
}

async function openaiComplete(cfg: LlmConfig, system: string, user: string): Promise<string> {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' }
  if (cfg.apiKey) headers.Authorization = `Bearer ${cfg.apiKey}`
  const data = await postJson(`${cleanBase(cfg.baseUrl)}/chat/completions`, headers, {
    model: cfg.model,
    messages: [
      { role: 'system', content: system },
      { role: 'user', content: user }
    ],
    stream: false
  })
  return data?.choices?.[0]?.message?.content ?? ''
}

async function anthropicComplete(cfg: LlmConfig, system: string, user: string): Promise<string> {
  if (!cfg.apiKey) throw new LlmError('Anthropic requires an API key. Open Settings.')
  const data = await postJson(
    `${cleanBase(cfg.baseUrl)}/messages`,
    {
      'Content-Type': 'application/json',
      'x-api-key': cfg.apiKey,
      'anthropic-version': '2023-06-01'
    },
    {
      model: cfg.model,
      system,
      max_tokens: 4096,
      messages: [{ role: 'user', content: user }]
    }
  )
  const parts = data?.content
  if (Array.isArray(parts)) return parts.map((p: any) => p?.text ?? '').join('')
  return ''
}

async function googleComplete(cfg: LlmConfig, system: string, user: string): Promise<string> {
  if (!cfg.apiKey) throw new LlmError('Google requires an API key. Open Settings.')
  const model = cfg.model.replace(/^models\//, '')
  const url = `${cleanBase(cfg.baseUrl)}/models/${model}:generateContent?key=${cfg.apiKey}`
  const data = await postJson(
    url,
    { 'Content-Type': 'application/json' },
    {
      systemInstruction: { parts: [{ text: system }] },
      contents: [{ role: 'user', parts: [{ text: user }] }]
    }
  )
  return data?.candidates?.[0]?.content?.parts?.[0]?.text ?? ''
}
