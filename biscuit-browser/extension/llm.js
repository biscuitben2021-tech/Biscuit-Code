// Minimal multi-provider chat client for the extension service worker. Defaults
// to an OpenAI-compatible local endpoint (LM Studio) so the agent runs free with
// no API cost. Same provider shapes as the Electron app's src/main/agent/llm.ts.

const api = globalThis.browser ?? globalThis.chrome

/** Read provider config from extension storage, with free-local defaults. */
export async function getConfig() {
  const d = await api.storage.local.get(['provider', 'baseUrl', 'model', 'apiKey', 'mode'])
  return {
    provider: d.provider || 'openai_compatible',
    baseUrl: d.baseUrl || 'http://localhost:1234/v1',
    model: d.model || 'local-model',
    apiKey: d.apiKey || '',
    mode: d.mode || 'assisted'
  }
}

/** Send a chat completion; returns the assistant text. Throws on HTTP/transport error. */
export async function chat(cfg, system, messages) {
  const base = (cfg.baseUrl || 'http://localhost:1234/v1').replace(/\/+$/, '')

  if (cfg.provider === 'anthropic') {
    const r = await fetch(base + '/messages', {
      method: 'POST',
      headers: { 'content-type': 'application/json', 'x-api-key': cfg.apiKey || '', 'anthropic-version': '2023-06-01' },
      body: JSON.stringify({ model: cfg.model, system, max_tokens: 1024, messages })
    })
    const j = await r.json().catch(() => ({}))
    if (!r.ok) throw new Error((j && j.error && j.error.message) || 'HTTP ' + r.status)
    return (j.content || []).map((b) => b.text || '').join('')
  }

  if (cfg.provider === 'google') {
    const model = (cfg.model || '').replace(/^models\//, '')
    const contents = messages.map((m) => ({
      role: m.role === 'assistant' ? 'model' : 'user',
      parts: [{ text: m.content }]
    }))
    const r = await fetch(base + '/models/' + model + ':generateContent', {
      method: 'POST',
      headers: { 'content-type': 'application/json', 'x-goog-api-key': cfg.apiKey || '' },
      body: JSON.stringify({ systemInstruction: { parts: [{ text: system }] }, contents })
    })
    const j = await r.json().catch(() => ({}))
    if (!r.ok) throw new Error((j && j.error && j.error.message) || 'HTTP ' + r.status)
    const parts = (j.candidates && j.candidates[0] && j.candidates[0].content && j.candidates[0].content.parts) || []
    return parts.map((p) => p.text || '').join('')
  }

  // openai | openai_compatible | lmstudio
  const headers = { 'content-type': 'application/json' }
  if (cfg.apiKey) headers['authorization'] = 'Bearer ' + cfg.apiKey
  const r = await fetch(base + '/chat/completions', {
    method: 'POST',
    headers,
    body: JSON.stringify({ model: cfg.model, messages: [{ role: 'system', content: system }, ...messages], stream: false })
  })
  const j = await r.json().catch(() => ({}))
  if (!r.ok) throw new Error((j && j.error && j.error.message) || 'HTTP ' + r.status)
  return (j.choices && j.choices[0] && j.choices[0].message && j.choices[0].message.content) || ''
}
