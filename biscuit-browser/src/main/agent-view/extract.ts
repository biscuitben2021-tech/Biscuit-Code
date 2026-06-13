// @ts-nocheck — extractorSource() below runs in the PAGE world (DOM globals),
// not in Node. It is only ever serialized via .toString(); disabling type
// checks here is intentional. buildExtractScript()'s public signature is still
// typed for callers.
//
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
//   live element with data-biscuit-ref so later clickRef/typeRef can re-find it
//   (the ref finder in browserActions walks shadow roots + same-origin frames).
// - We stamp documentElement with data-biscuit-gen = the generation. After a
//   navigation/reload the document is replaced, so old refs naturally disappear
//   (actions then report "ref expired -> refreshAgentView").
//
// Coverage (V1): main document, OPEN shadow roots (recursively), and
// same-origin iframes/frames (recursively, depth-capped). It also surfaces
// elements that behave like buttons via cursor:pointer even without a role.
// It CANNOT see closed shadow roots, cross-origin frames, or <canvas>-painted
// UIs — those are reported in `context.notes` rather than silently dropped.

export interface ExtractOptions {
  generation: number
  maxElements: number
  maxTextChars: number
}

export function buildExtractScript(opts: ExtractOptions): string {
  const { generation, maxElements, maxTextChars } = opts
  return `(${extractorSource.toString()})(${generation}, ${maxElements}, ${maxTextChars})`
}

// Exported for unit testing against a jsdom document. In production it is only
// ever serialized via .toString() and run in the page world; type annotations
// are stripped at compile time.
export function extractorSource(generation: number, maxElements: number, maxTextChars: number): unknown {
  const topDoc = document
  const topWin = window
  topDoc.documentElement.setAttribute('data-biscuit-gen', String(generation))

  const SELECTOR =
    'a[href], button, input, textarea, select, summary, ' +
    '[role="button"], [role="link"], [role="checkbox"], [role="radio"], ' +
    '[role="textbox"], [role="searchbox"], [role="combobox"], [role="switch"], ' +
    '[role="menuitem"], [role="menuitemcheckbox"], [role="menuitemradio"], ' +
    '[role="option"], [role="tab"], [contenteditable=""], [contenteditable="true"], [onclick]'

  const NODE_CAP = 12000 // total elements walked across all roots
  const CURSOR_CAP = 1500 // bound the (expensive) getComputedStyle pointer scan
  const FRAME_DEPTH = 4

  const frames = { total: 0, sameOrigin: 0, crossOrigin: 0 }
  let shadowRoots = 0
  let depthCapped = 0 // same-origin frames skipped only because of the depth limit
  let nodesWalked = 0
  let cursorChecks = 0
  let truncated = false

  const elements = []
  let counter = 0

  function txt(el) {
    return (el.innerText || el.textContent || '').replace(/\s+/g, ' ').trim()
  }

  function styleOf(el, win) {
    try {
      const s = win.getComputedStyle(el)
      if (!s) return null
      if (s.display === 'none' || s.visibility === 'hidden') return null
      if (parseFloat(s.opacity || '1') === 0) return null
      return s
    } catch (e) {
      return null
    }
  }

  function inViewport(rect, win) {
    const vw = win.innerWidth || 0
    const vh = win.innerHeight || 0
    return rect.bottom > 0 && rect.right > 0 && rect.top < vh && rect.left < vw
  }

  function labelFor(el, scope) {
    const aria = el.getAttribute('aria-label')
    if (aria && aria.trim()) return aria.trim()
    const labelledby = el.getAttribute('aria-labelledby')
    if (labelledby) {
      const parts = labelledby
        .split(/\s+/)
        .map(function (id) {
          try {
            return scope.getElementById ? scope.getElementById(id) : null
          } catch (e) {
            return null
          }
        })
        .filter(Boolean)
        .map(function (n) {
          return txt(n)
        })
        .join(' ')
        .trim()
      if (parts) return parts
    }
    const id = el.getAttribute('id')
    if (id) {
      try {
        const esc = window.CSS && window.CSS.escape ? window.CSS.escape(id) : id
        const lbl = scope.querySelector('label[for="' + esc + '"]')
        if (lbl && txt(lbl)) return txt(lbl)
      } catch (e) {}
    }
    const placeholder = el.getAttribute('placeholder')
    if (placeholder && placeholder.trim()) return placeholder.trim()
    const title = el.getAttribute('title')
    if (title && title.trim()) return title.trim()
    const alt = el.getAttribute('alt')
    if (alt && alt.trim()) return alt.trim()
    const t = txt(el)
    if (t) return t.slice(0, 140)
    // Never surface a field's value as its label for sensitive inputs — that
    // would leak a password / card number into the name sent to the model.
    const val = el.value
    if (val && !isSensitive(el)) return String(val).slice(0, 140)
    const name = el.getAttribute('name')
    return name ? name.trim() : ''
  }

  function roleFor(el) {
    const tag = el.tagName.toLowerCase()
    const explicit = el.getAttribute('role')
    if (explicit === 'link') return 'link'
    if (explicit === 'button') return 'button'
    if (explicit === 'checkbox' || explicit === 'switch' || explicit === 'menuitemcheckbox') return 'checkbox'
    if (explicit === 'radio' || explicit === 'menuitemradio') return 'radio'
    if (explicit === 'textbox' || explicit === 'searchbox' || explicit === 'combobox') return 'textbox'
    if (tag === 'a' && el.href) return 'link'
    if (tag === 'button' || tag === 'summary') return 'button'
    if (tag === 'select') return 'select'
    if (tag === 'textarea') return 'textbox'
    if (tag === 'input') {
      const t = (el.type || 'text').toLowerCase()
      if (t === 'submit' || t === 'button' || t === 'image' || t === 'reset') return 'submit'
      if (t === 'checkbox') return 'checkbox'
      if (t === 'radio') return 'radio'
      return 'textbox'
    }
    if (el.isContentEditable) return 'textbox'
    return 'other'
  }

  function isSensitive(el) {
    const t = (el.type || '').toLowerCase()
    if (t === 'password') return true
    const hay = (
      (el.name || '') +
      ' ' +
      (el.id || '') +
      ' ' +
      (el.getAttribute('autocomplete') || '')
    ).toLowerCase()
    return /pass|card|cc-|cvv|cvc|ssn|secret|otp|securitycode|account-?number|routing/.test(hay)
  }

  function disabledLooking(el, style) {
    if (el.disabled === true) return true
    try {
      if (el.getAttribute && el.getAttribute('aria-disabled') === 'true') return true
    } catch (e) {}
    if (style && style.pointerEvents === 'none') return true
    return false
  }

  // Containment across shadow boundaries — Node.contains() does not pierce
  // host->shadow, so a same-widget hit would otherwise read as "covered".
  function composedContains(a, b) {
    let n = b
    while (n) {
      if (n === a) return true
      if (n.nodeType === 11 && n.host) {
        n = n.host
        continue
      }
      n = n.parentNode
    }
    return false
  }

  function coveredAt(el, root, rect, win) {
    const cx = rect.left + rect.width / 2
    const cy = rect.top + rect.height / 2
    if (cx < 0 || cy < 0 || cx > (win.innerWidth || 0) || cy > (win.innerHeight || 0)) return false
    let top = null
    try {
      top = root.elementFromPoint ? root.elementFromPoint(cx, cy) : null
    } catch (e) {
      return false
    }
    if (!top || top === el) return false
    try {
      if (composedContains(el, top) || composedContains(top, el)) return false
    } catch (e) {}
    return true
  }

  function add(el, ctx, roleOverride, style) {
    if (counter >= maxElements) {
      truncated = true
      return
    }
    const s = style || styleOf(el, ctx.win)
    if (!s) return
    const rect = el.getBoundingClientRect()
    if (rect.width <= 1 && rect.height <= 1) return
    counter += 1
    const ref = 'e' + counter
    el.setAttribute('data-biscuit-ref', ref)
    const tag = (el.tagName || '').toLowerCase()
    const item = {
      ref: '@' + ref,
      role: roleOverride || roleFor(el),
      tag: tag,
      name: labelFor(el, ctx.root),
      state: {
        visible: true,
        enabled: !disabledLooking(el, s),
        inViewport: inViewport(rect, ctx.win)
      },
      box: {
        x: Math.round(rect.left),
        y: Math.round(rect.top),
        width: Math.round(rect.width),
        height: Math.round(rect.height)
      }
    }
    if (ctx.source !== 'dom') item.via = ctx.source
    if (coveredAt(el, ctx.root, rect, ctx.win)) item.state.covered = true
    if (tag === 'a' && el.href) item.href = String(el.href)
    if (tag === 'input' || tag === 'button') item.type = (el.type || '').toLowerCase()
    if (tag === 'input' || tag === 'textarea' || tag === 'select') {
      // Sensitive fields are flagged but their value is NEVER captured (it would
      // be sent to the model / shown in the UI). Non-sensitive values are kept.
      if (isSensitive(el)) item.sensitive = true
      else if (typeof el.value === 'string') item.value = el.value.slice(0, 200)
      if (el.type === 'checkbox' || el.type === 'radio') item.state.checked = el.checked === true
    }
    try {
      if (ctx.doc.activeElement === el) item.state.focused = true
    } catch (e) {}
    elements.push(item)
  }

  function walkRoot(root, ctx) {
    let all
    try {
      all = root.querySelectorAll('*')
    } catch (e) {
      return
    }
    // Clear stale refs within THIS root's scope before re-tagging. querySelectorAll
    // does not pierce shadow boundaries, so each shadow root is cleared by its own
    // walkRoot call — this prevents a hidden/skipped element from a prior extraction
    // from keeping a ref that collides with a fresh one (which would misclick).
    try {
      const stale = root.querySelectorAll('[data-biscuit-ref]')
      for (let s = 0; s < stale.length; s++) stale[s].removeAttribute('data-biscuit-ref')
    } catch (e) {
      /* ignore */
    }
    for (let i = 0; i < all.length; i++) {
      if (nodesWalked >= NODE_CAP) {
        truncated = true
        return
      }
      nodesWalked++
      const el = all[i]

      if (el.shadowRoot) {
        shadowRoots++
        walkRoot(el.shadowRoot, {
          win: ctx.win,
          doc: ctx.doc,
          root: el.shadowRoot,
          source: ctx.source === 'iframe' ? 'iframe' : 'shadow',
          depth: ctx.depth
        })
      }

      const tag = el.tagName
      if (tag === 'IFRAME' || tag === 'FRAME') {
        handleFrame(el, ctx)
        continue
      }

      let matched = false
      try {
        matched = el.matches(SELECTOR)
      } catch (e) {
        matched = false
      }
      if (matched) {
        add(el, ctx, null, null)
        continue
      }

      // Heuristic: elements that behave like buttons via cursor:pointer even
      // without a role/onclick attribute (the "div with addEventListener" case).
      if (cursorChecks < CURSOR_CAP) {
        cursorChecks++
        const style = styleOf(el, ctx.win)
        if (style && style.cursor === 'pointer') {
          const label = txt(el)
          if (label && label.length <= 60) {
            let skip = false
            try {
              skip = !!(
                el.closest(SELECTOR) ||
                el.querySelector(SELECTOR) ||
                el.closest('[data-biscuit-ref]')
              )
            } catch (e) {
              skip = false
            }
            if (!skip) add(el, ctx, 'button', style)
          }
        }
      }
    }
  }

  function handleFrame(frameEl, ctx) {
    frames.total++
    if (ctx.depth >= FRAME_DEPTH) {
      // Not necessarily cross-origin — just deeper than we descend. Track it
      // separately so we don't claim the agent is "blind" to a reachable frame.
      depthCapped++
      return
    }
    let cdoc = null
    let cwin = null
    try {
      cdoc = frameEl.contentDocument
      cwin = frameEl.contentWindow
    } catch (e) {
      cdoc = null
    }
    if (cdoc && cdoc.documentElement && cwin) {
      frames.sameOrigin++
      try {
        const st = cdoc.querySelectorAll('[data-biscuit-ref]')
        for (let j = 0; j < st.length; j++) st[j].removeAttribute('data-biscuit-ref')
      } catch (e) {}
      // Stamp the frame's generation too, so a ref found inside it can be
      // validated by clickRef/typeRef even though the top guard only sees the
      // top document. A frame not reached this round keeps its old stamp and its
      // stale refs are correctly rejected.
      try {
        cdoc.documentElement.setAttribute('data-biscuit-gen', String(generation))
      } catch (e) {}
      walkRoot(cdoc, { win: cwin, doc: cdoc, root: cdoc, source: 'iframe', depth: ctx.depth + 1 })
    } else {
      frames.crossOrigin++
    }
  }

  // Clear stale refs in the top document so they cannot be reused.
  try {
    const stale = topDoc.querySelectorAll('[data-biscuit-ref]')
    for (let s = 0; s < stale.length; s++) stale[s].removeAttribute('data-biscuit-ref')
  } catch (e) {}

  walkRoot(topDoc, { win: topWin, doc: topDoc, root: topDoc, source: 'dom', depth: 0 })

  const headings = []
  const hNodes = topDoc.querySelectorAll('h1, h2, h3, h4, h5, h6')
  for (let i = 0; i < hNodes.length; i++) {
    if (headings.length >= 60) break
    const h = hNodes[i]
    if (!styleOf(h, topWin)) continue
    const text = txt(h)
    if (text) headings.push({ level: parseInt(h.tagName.substring(1), 10), text: text.slice(0, 200) })
  }

  let text = ((topDoc.body && topDoc.body.innerText) || '')
    .replace(/[\t\r]+/g, ' ')
    .replace(/\n{3,}/g, '\n\n')
    .trim()
  if (text.length > maxTextChars) {
    text = text.slice(0, maxTextChars)
    truncated = true
  }

  const notes = []
  if (frames.crossOrigin > 0)
    notes.push(
      frames.crossOrigin + ' cross-origin frame(s) could not be inspected (the agent is blind to these)'
    )
  if (depthCapped > 0)
    notes.push(
      depthCapped + ' frame(s) beyond max nesting depth were not inspected (depth limit, not cross-origin)'
    )
  if (shadowRoots > 0) notes.push(shadowRoots + ' open shadow root(s) traversed')
  if (truncated) notes.push('snapshot truncated (element/node cap reached)')

  return {
    url: location.href,
    title: topDoc.title,
    headings: headings,
    elements: elements,
    text: text,
    truncated: truncated,
    context: { frames: frames, shadowRoots: shadowRoots, notes: notes }
  }
}
