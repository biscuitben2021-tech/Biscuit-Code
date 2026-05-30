// @ts-nocheck — extractorSource() below runs in the PAGE world (DOM globals),
// not in Node. It is only ever serialized via .toString(); disabling type
// checks here is intentional. buildExtractScript()'s public signature is still
// typed for callers.
// The Agent View extractor. This source string is injected into the browsed
// page via webContents.executeJavaScript and runs in the PAGE world. The
// `extractorSource` function is dependency-free and self-contained so it can be
// serialized with .toString() and evaluated in the page. Main wraps the result
// with tabId/generation/timestamp.
//
// Design notes:
// - By default we do NOT send raw HTML or screenshots to the model. We extract
//   visible text + interactive elements only.
// - Each interactive element gets a short stable ref (@e1, @e2, ...). We tag the
//   live element with data-biscuit-ref so later clickRef/typeRef can re-find it.
// - We stamp documentElement with data-biscuit-gen = the generation. After a
//   navigation/reload the document is replaced, so old refs naturally disappear
//   (actions then report "ref expired -> refreshAgentView").

export interface ExtractOptions {
  generation: number
  maxElements: number
  maxTextChars: number
}

export function buildExtractScript(opts: ExtractOptions): string {
  const { generation, maxElements, maxTextChars } = opts
  return `(${extractorSource.toString()})(${generation}, ${maxElements}, ${maxTextChars})`
}

// Keep this function dependency-free; it is serialized via .toString() and run
// in the page world. Type annotations are stripped at compile time.
function extractorSource(generation: number, maxElements: number, maxTextChars: number): unknown {
  // @ts-nocheck
  /* eslint-disable */
  const doc = document
  const root = doc.documentElement

  // Clear refs from a previous generation so they cannot be reused.
  const stale = doc.querySelectorAll('[data-biscuit-ref]')
  for (let i = 0; i < stale.length; i++) stale[i].removeAttribute('data-biscuit-ref')
  root.setAttribute('data-biscuit-gen', String(generation))

  const vw = window.innerWidth || doc.clientWidth || 0
  const vh = window.innerHeight || doc.clientHeight || 0

  function isVisible(el) {
    const style = window.getComputedStyle(el)
    if (!style || style.display === 'none' || style.visibility === 'hidden') return false
    if (parseFloat(style.opacity || '1') === 0) return false
    const rect = el.getBoundingClientRect()
    if (rect.width <= 1 && rect.height <= 1) return false
    return true
  }

  function inViewport(rect) {
    return rect.bottom > 0 && rect.right > 0 && rect.top < vh && rect.left < vw
  }

  function textOf(el) {
    return (el.innerText || el.textContent || '').replace(/\s+/g, ' ').trim()
  }

  function labelFor(el) {
    const aria = el.getAttribute('aria-label')
    if (aria && aria.trim()) return aria.trim()
    const labelledby = el.getAttribute('aria-labelledby')
    if (labelledby) {
      const parts = labelledby
        .split(/\s+/)
        .map(function (id) { return doc.getElementById(id) })
        .filter(Boolean)
        .map(function (n) { return textOf(n) })
        .join(' ')
        .trim()
      if (parts) return parts
    }
    const id = el.getAttribute('id')
    if (id) {
      try {
        const esc = window.CSS && window.CSS.escape ? window.CSS.escape(id) : id
        const lbl = doc.querySelector('label[for="' + esc + '"]')
        if (lbl && textOf(lbl)) return textOf(lbl)
      } catch (e) {}
    }
    const placeholder = el.getAttribute('placeholder')
    if (placeholder && placeholder.trim()) return placeholder.trim()
    const title = el.getAttribute('title')
    if (title && title.trim()) return title.trim()
    const alt = el.getAttribute('alt')
    if (alt && alt.trim()) return alt.trim()
    const t = textOf(el)
    if (t) return t.slice(0, 140)
    const val = el.value
    if (val) return String(val).slice(0, 140)
    const name = el.getAttribute('name')
    return name ? name.trim() : ''
  }

  function roleFor(el) {
    const tag = el.tagName.toLowerCase()
    const explicit = el.getAttribute('role')
    if (explicit === 'link') return 'link'
    if (explicit === 'button') return 'button'
    if (explicit === 'checkbox') return 'checkbox'
    if (explicit === 'radio') return 'radio'
    if (explicit === 'textbox' || explicit === 'searchbox') return 'textbox'
    if (tag === 'a' && el.href) return 'link'
    if (tag === 'button') return 'button'
    if (tag === 'select') return 'select'
    if (tag === 'textarea') return 'textbox'
    if (tag === 'input') {
      const t = (el.type || 'text').toLowerCase()
      if (t === 'submit' || t === 'button' || t === 'image') return 'submit'
      if (t === 'checkbox') return 'checkbox'
      if (t === 'radio') return 'radio'
      return 'textbox'
    }
    return 'other'
  }

  function isSensitive(el) {
    const t = (el.type || '').toLowerCase()
    if (t === 'password') return true
    const hay = ((el.name || '') + ' ' + (el.id || '') + ' ' + (el.getAttribute('autocomplete') || '')).toLowerCase()
    return /pass|card|cc-|cvv|cvc|ssn|secret|otp|securitycode|account-?number|routing/.test(hay)
  }

  const selector =
    'a[href], button, input, textarea, select, [role="button"], [role="link"], [role="checkbox"], [role="textbox"], [onclick]'
  const nodes = doc.querySelectorAll(selector)

  const elements = []
  let truncated = false
  let counter = 0
  for (let i = 0; i < nodes.length; i++) {
    const el = nodes[i]
    if (counter >= maxElements) {
      truncated = true
      break
    }
    if (!isVisible(el)) continue
    const rect = el.getBoundingClientRect()
    counter += 1
    const ref = 'e' + counter
    el.setAttribute('data-biscuit-ref', ref)
    const tag = el.tagName.toLowerCase()
    const item = {
      ref: '@' + ref,
      role: roleFor(el),
      tag: tag,
      name: labelFor(el),
      state: {
        visible: true,
        enabled: el.disabled !== true,
        inViewport: inViewport(rect)
      },
      box: {
        x: Math.round(rect.left),
        y: Math.round(rect.top),
        width: Math.round(rect.width),
        height: Math.round(rect.height)
      }
    }
    if (tag === 'a') item.href = el.href
    if (tag === 'input' || tag === 'button') item.type = (el.type || '').toLowerCase()
    if (tag === 'input' || tag === 'textarea' || tag === 'select') {
      if (typeof el.value === 'string') item.value = el.value.slice(0, 200)
      if (el.type === 'checkbox' || el.type === 'radio') item.state.checked = el.checked === true
      if (isSensitive(el)) item.sensitive = true
    }
    if (doc.activeElement === el) item.state.focused = true
    elements.push(item)
  }

  const headings = []
  const hNodes = doc.querySelectorAll('h1, h2, h3, h4, h5, h6')
  for (let i = 0; i < hNodes.length; i++) {
    if (headings.length >= 60) break
    const h = hNodes[i]
    if (!isVisible(h)) continue
    const text = textOf(h)
    if (text) headings.push({ level: parseInt(h.tagName.substring(1), 10), text: text.slice(0, 200) })
  }

  let text = ((doc.body && doc.body.innerText) || '').replace(/[\t\r]+/g, ' ').replace(/\n{3,}/g, '\n\n').trim()
  if (text.length > maxTextChars) {
    text = text.slice(0, maxTextChars)
    truncated = true
  }

  return { url: location.href, title: doc.title, headings: headings, elements: elements, text: text, truncated: truncated }
  /* eslint-enable */
}
