// Biscuit Agent content script (runs in the page world; injected on demand).
// Self-contained — no imports. Adapts the Agent View extractor + page actions
// from the Electron app (src/main/agent-view/extract.ts and
// src/main/actions/browserActions.ts) to a Chrome/Safari content script.
(() => {
  if (window.__biscuitAgentInstalled) return
  window.__biscuitAgentInstalled = true

  const MAX_ELEMENTS = 150
  const MAX_TEXT = 6000
  // Fields whose VALUES must never be captured/sent (password, card, etc.).
  const SENSITIVE = /pass|card\b|cc-|cvv|cvc|ssn|secret|otp|security.?code|account.?number/i

  function isVisible(el) {
    let s
    try {
      s = window.getComputedStyle(el)
    } catch {
      return false
    }
    if (!s || s.visibility === 'hidden' || s.display === 'none' || parseFloat(s.opacity || '1') === 0) {
      return false
    }
    const r = el.getBoundingClientRect()
    return r.width > 1 && r.height > 1
  }

  function roleOf(el) {
    const tag = el.tagName.toLowerCase()
    const role = (el.getAttribute('role') || '').toLowerCase()
    if (tag === 'a') return 'link'
    if (tag === 'button' || role === 'button') return 'button'
    if (tag === 'select') return 'select'
    if (tag === 'textarea') return 'textbox'
    if (tag === 'input') {
      const t = (el.getAttribute('type') || 'text').toLowerCase()
      if (t === 'checkbox') return 'checkbox'
      if (t === 'radio') return 'radio'
      if (t === 'submit' || t === 'button' || t === 'image') return 'submit'
      return 'textbox'
    }
    if (el.isContentEditable) return 'textbox'
    if (['link', 'checkbox', 'radio', 'tab', 'menuitem', 'switch'].includes(role)) return role
    return 'other'
  }

  function isInteractive(el) {
    const tag = el.tagName.toLowerCase()
    if (['a', 'button', 'input', 'select', 'textarea'].includes(tag)) return true
    const role = (el.getAttribute('role') || '').toLowerCase()
    if (['button', 'link', 'checkbox', 'radio', 'tab', 'menuitem', 'switch'].includes(role)) return true
    if (el.hasAttribute('onclick') || el.isContentEditable) return true
    try {
      // Buttons-by-cursor: only leaf-ish pointer elements, to avoid tagging
      // every wrapping div on hover-styled pages.
      if (window.getComputedStyle(el).cursor === 'pointer' && el.children.length <= 3) return true
    } catch {
      /* ignore */
    }
    return false
  }

  function labelOf(el) {
    const aria = el.getAttribute('aria-label')
    if (aria && aria.trim()) return aria.trim().slice(0, 100)
    const txt = (
      el.innerText ||
      el.getAttribute('placeholder') ||
      el.getAttribute('name') ||
      el.getAttribute('title') ||
      el.alt ||
      ''
    )
      .replace(/\s+/g, ' ')
      .trim()
    return txt.slice(0, 100)
  }

  function isSensitive(el) {
    if (el.type === 'password') return true
    const hay = `${el.name || ''} ${el.id || ''} ${el.getAttribute('autocomplete') || ''} ${
      el.getAttribute('aria-label') || ''
    }`
    return SENSITIVE.test(hay)
  }

  function extract() {
    const generation = (parseInt(document.documentElement.getAttribute('data-biscuit-gen') || '0', 10) || 0) + 1
    document.documentElement.setAttribute('data-biscuit-gen', String(generation))
    const elements = []
    let counter = 0
    const seen = new Set()

    function walk(root) {
      let all
      try {
        all = root.querySelectorAll('*')
      } catch {
        return
      }
      // Clear stale refs within THIS root scope (querySelectorAll does not pierce
      // shadow boundaries, so each shadow root is cleared by its own walk).
      try {
        root.querySelectorAll('[data-biscuit-ref]').forEach((e) => e.removeAttribute('data-biscuit-ref'))
      } catch {
        /* ignore */
      }
      for (const el of all) {
        if (counter >= MAX_ELEMENTS) return
        if (el.shadowRoot) walk(el.shadowRoot)
        if (seen.has(el) || !isInteractive(el) || !isVisible(el)) continue
        seen.add(el)
        counter += 1
        const ref = 'e' + counter
        el.setAttribute('data-biscuit-ref', ref)
        const item = { ref: '@' + ref, role: roleOf(el), label: labelOf(el) }
        if (el.disabled) item.disabled = true
        if (isSensitive(el)) item.sensitive = true
        else if (el.value && el.tagName !== 'SELECT') item.value = String(el.value).slice(0, 80)
        if (document.activeElement === el) item.focused = true
        elements.push(item)
      }
    }
    walk(document)

    let text = ''
    try {
      text = (document.body ? document.body.innerText : '').replace(/[ \t]+/g, ' ').trim().slice(0, MAX_TEXT)
    } catch {
      /* ignore */
    }
    return {
      ok: true,
      url: location.href,
      title: document.title,
      generation,
      elements,
      text,
      truncated: counter >= MAX_ELEMENTS
    }
  }

  // Resolve a ref across the main document + open shadow roots (matches extract).
  function find(ref) {
    const bare = ref.replace(/^@/, '')
    function search(root) {
      let hit = null
      try {
        hit = root.querySelector('[data-biscuit-ref="' + bare + '"]')
      } catch {
        /* ignore */
      }
      if (hit) return hit
      let all
      try {
        all = root.querySelectorAll('*')
      } catch {
        return null
      }
      for (const el of all) {
        if (el.shadowRoot) {
          const r = search(el.shadowRoot)
          if (r) return r
        }
      }
      return null
    }
    return search(document)
  }

  function genOk(gen) {
    return document.documentElement.getAttribute('data-biscuit-gen') === String(gen)
  }

  function clickRef(ref, gen) {
    if (!genOk(gen)) return { ok: false, detail: 'refs expired (page changed) — refreshAgentView' }
    const el = find(ref)
    if (!el) return { ok: false, detail: 'ref ' + ref + ' not found — refreshAgentView' }
    try {
      el.scrollIntoView({ block: 'center', inline: 'center' })
    } catch {
      /* ignore */
    }
    const label = labelOf(el)
    try {
      el.click()
    } catch (e) {
      return { ok: false, detail: 'click failed: ' + e.message }
    }
    return { ok: true, detail: 'clicked ' + ref + (label ? ' (' + label + ')' : '') }
  }

  function typeRef(ref, value, gen) {
    if (!genOk(gen)) return { ok: false, detail: 'refs expired (page changed) — refreshAgentView' }
    const el = find(ref)
    if (!el) return { ok: false, detail: 'ref ' + ref + ' not found — refreshAgentView' }
    try {
      el.focus()
    } catch {
      /* ignore */
    }
    try {
      if (el.tagName === 'SELECT') {
        let matched = -1
        for (let i = 0; i < el.options.length; i++) {
          const o = el.options[i]
          if (o.value === value || (o.text || '').trim() === value) {
            matched = i
            break
          }
        }
        if (matched === -1) return { ok: false, detail: 'no <select> option matches "' + value + '"' }
        el.selectedIndex = matched
      } else if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {
        const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype
        const desc = Object.getOwnPropertyDescriptor(proto, 'value')
        if (desc && desc.set) desc.set.call(el, value)
        else el.value = value
      } else if (el.isContentEditable) {
        el.textContent = value
      } else {
        return { ok: false, detail: ref + ' is not an editable field' }
      }
      el.dispatchEvent(new Event('input', { bubbles: true }))
      el.dispatchEvent(new Event('change', { bubbles: true }))
    } catch (e) {
      return { ok: false, detail: 'type failed: ' + e.message }
    }
    return {
      ok: true,
      detail: el.tagName === 'SELECT' ? 'selected option in ' + ref : 'typed ' + value.length + ' chars into ' + ref
    }
  }

  function scrollPage(direction, pages) {
    const h = window.innerHeight || 800
    const n = Math.max(1, Math.floor(pages || 1))
    if (direction === 'top') {
      window.scrollTo(0, 0)
      return { ok: true, detail: 'scrolled to top' }
    }
    if (direction === 'bottom') {
      window.scrollTo(0, document.body ? document.body.scrollHeight : 0)
      return { ok: true, detail: 'scrolled to bottom' }
    }
    window.scrollBy(0, (direction === 'up' ? -1 : 1) * n * h * 0.9)
    return { ok: true, detail: 'scrolled ' + direction + ' ' + n + ' page(s)' }
  }

  const api = globalThis.browser ?? globalThis.chrome
  api.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
    if (!msg || msg.target !== 'biscuit-content') return undefined
    try {
      if (msg.type === 'view') sendResponse(extract())
      else if (msg.type === 'click') sendResponse(clickRef(msg.ref, msg.gen))
      else if (msg.type === 'type') sendResponse(typeRef(msg.ref, msg.text, msg.gen))
      else if (msg.type === 'scroll') sendResponse(scrollPage(msg.direction, msg.pages))
      else sendResponse({ ok: false, detail: 'unknown action' })
    } catch (e) {
      sendResponse({ ok: false, detail: 'content error: ' + e.message })
    }
    return true
  })
})()
