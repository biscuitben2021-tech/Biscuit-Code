/**
 * @vitest-environment jsdom
 */
import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest'
import { extractorSource } from '../src/main/agent-view/extract'

interface RawView {
  url: string
  title: string
  headings: { level: number; text: string }[]
  elements: any[]
  text: string
  truncated: boolean
  context: { frames: { total: number; sameOrigin: number; crossOrigin: number }; shadowRoots: number; notes: string[] }
}

// jsdom does no layout, so getBoundingClientRect returns zeros and every element
// would be filtered as "invisible". Give elements a usable box for the duration
// of these tests.
const origRect = Element.prototype.getBoundingClientRect
beforeAll(() => {
  Element.prototype.getBoundingClientRect = function (): DOMRect {
    return { x: 10, y: 10, left: 10, top: 10, right: 110, bottom: 30, width: 100, height: 20, toJSON: () => ({}) } as DOMRect
  }
})
afterAll(() => {
  Element.prototype.getBoundingClientRect = origRect
})
beforeEach(() => {
  document.documentElement.removeAttribute('data-biscuit-gen')
  document.body.innerHTML = ''
})

function run(): RawView {
  return extractorSource(7, 150, 6000) as RawView
}
function byName(v: RawView, name: string): any | undefined {
  return v.elements.find((e) => e.name === name)
}

describe('extractorSource (Agent View extraction)', () => {
  it('stamps the generation on documentElement', () => {
    document.body.innerHTML = '<button>Go</button>'
    run()
    expect(document.documentElement.getAttribute('data-biscuit-gen')).toBe('7')
  })

  it('extracts interactive elements with stable @e refs and roles', () => {
    document.body.innerHTML = `
      <a href="https://x.com/docs">Documentation</a>
      <button>Submit</button>
      <input type="text" name="q" />
      <div role="button">Menu</div>
    `
    const v = run()
    const link = byName(v, 'Documentation')
    const button = byName(v, 'Submit')
    const menu = byName(v, 'Menu')

    expect(link?.role).toBe('link')
    expect(link?.href).toMatch(/x\.com\/docs/)
    expect(button?.role).toBe('button')
    expect(menu?.role).toBe('button')

    // Every element gets a unique @e ref and a live data-biscuit-ref tag.
    for (const e of v.elements) expect(e.ref).toMatch(/^@e\d+$/)
    expect(document.querySelectorAll('[data-biscuit-ref]').length).toBe(v.elements.length)
  })

  it('detects sensitive fields (password + payment field names)', () => {
    document.body.innerHTML = `
      <input type="password" name="pw" aria-label="Password" />
      <input type="text" name="cardNumber" aria-label="Card" />
      <input type="text" name="q" aria-label="Search" />
    `
    const v = run()
    expect(byName(v, 'Password')?.sensitive).toBe(true)
    expect(byName(v, 'Card')?.sensitive).toBe(true)
    expect(byName(v, 'Search')?.sensitive).toBeUndefined()
  })

  it('never leaks a sensitive field value into name or value', () => {
    // A prefilled password with no label/aria/placeholder previously fell back
    // to the value as its label — leaking the secret to the model + UI.
    document.body.innerHTML = `<input type="password" name="pw" value="hunter2-secret" />`
    const v = run()
    const pw = v.elements.find((e) => e.sensitive)
    expect(pw).toBeTruthy()
    expect(pw.name).not.toContain('hunter2')
    expect(pw.name).toBe('pw') // falls back to the name attribute, not the value
    expect(pw.value).toBeUndefined()
  })

  it('reports checkbox state', () => {
    document.body.innerHTML = `<input type="checkbox" aria-label="Agree" checked />`
    const cb = byName(run(), 'Agree')
    expect(cb?.role).toBe('checkbox')
    expect(cb?.state.checked).toBe(true)
  })

  it('skips elements hidden via display:none', () => {
    document.body.innerHTML = `
      <button>Visible</button>
      <button style="display:none">Hidden</button>
    `
    const v = run()
    expect(byName(v, 'Visible')).toBeTruthy()
    expect(byName(v, 'Hidden')).toBeUndefined()
  })

  it('traverses open shadow roots and tags their elements as via:shadow', () => {
    const host = document.createElement('div')
    document.body.appendChild(host)
    const root = host.attachShadow({ mode: 'open' })
    root.innerHTML = `<button>ShadowButton</button>`

    const v = run()
    const sb = byName(v, 'ShadowButton')
    expect(sb).toBeTruthy()
    expect(sb.via).toBe('shadow')
    expect(v.context.shadowRoots).toBeGreaterThanOrEqual(1)
  })

  it('always returns the honest context block', () => {
    document.body.innerHTML = '<button>Go</button>'
    const v = run()
    expect(v.context).toBeDefined()
    expect(v.context.frames).toEqual({ total: 0, sameOrigin: 0, crossOrigin: 0 })
    expect(Array.isArray(v.context.notes)).toBe(true)
  })
})
