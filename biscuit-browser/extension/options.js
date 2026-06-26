// Biscuit Agent settings page. Persists provider config to extension storage.
const api = globalThis.browser ?? globalThis.chrome

const fields = ['provider', 'baseUrl', 'model', 'apiKey', 'mode']
const el = (id) => document.getElementById(id)

api.storage.local.get(fields).then((d) => {
  if (d.provider) el('provider').value = d.provider
  el('baseUrl').value = d.baseUrl || 'http://localhost:1234/v1'
  el('model').value = d.model || 'local-model'
  el('apiKey').value = d.apiKey || ''
  if (d.mode) el('mode').value = d.mode
})

el('save').addEventListener('click', async () => {
  await api.storage.local.set({
    provider: el('provider').value,
    baseUrl: el('baseUrl').value.trim(),
    model: el('model').value.trim(),
    apiKey: el('apiKey').value,
    mode: el('mode').value
  })
  const saved = el('saved')
  saved.hidden = false
  setTimeout(() => (saved.hidden = true), 1500)
})
