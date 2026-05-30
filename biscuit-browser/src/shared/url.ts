// Pure URL normalization shared by the tab manager (address bar) and the agent.
// Kept free of any Electron/Node import so it is trivially unit-testable and can
// be reused in any process.

const SEARCH_PREFIX = 'https://duckduckgo.com/?q='

// Schemes that can read local files, run script, or escape the http(s) sandbox.
// Neither the address bar nor the agent may navigate to these — they are routed
// to a web search instead.
const BLOCKED_SCHEME = /^(file|data|javascript|blob|chrome|about|view-source):/i

/** Build a web-search URL for arbitrary text. */
export function searchUrl(query: string): string {
  return `${SEARCH_PREFIX}${encodeURIComponent(query)}`
}

/**
 * Turn user/address-bar or agent input into a loadable URL.
 *
 * Only `http(s)` and `about:blank` are navigable. Anything that looks like a
 * blocked scheme (file:, data:, javascript:, blob:, chrome:, view-source:, or
 * other about: targets) is sent to a web search rather than loaded, so neither
 * the agent nor the address bar can reach the local filesystem or execute
 * inline script. Schemeless domain-like input is upgraded to https; everything
 * else is treated as a search query.
 */
export function normalizeUrl(input: string): string {
  const trimmed = (input ?? '').trim()
  if (!trimmed) return searchUrl('')
  if (trimmed.toLowerCase() === 'about:blank') return 'about:blank'
  if (BLOCKED_SCHEME.test(trimmed)) return searchUrl(trimmed)
  if (/^https?:\/\//i.test(trimmed)) return trimmed
  // Schemeless: treat domain-like input (no spaces, has a dot + TLD) as https,
  // otherwise search.
  if (!trimmed.includes(' ') && /^[^\s]+\.[^\s]{2,}(\/.*)?$/.test(trimmed)) {
    return `https://${trimmed}`
  }
  return searchUrl(trimmed)
}
