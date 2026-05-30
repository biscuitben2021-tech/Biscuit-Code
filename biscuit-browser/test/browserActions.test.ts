import { describe, it, expect } from 'vitest'
import { clickRef, typeRef } from '../src/main/actions/browserActions'

// These tests inspect the page-world script that browserActions builds (without
// running Electron). The key invariants are about what can leak into the result
// detail — which is logged, shown in the UI, and fed back to the model.
function mockWc() {
  let captured = ''
  const wc = {
    executeJavaScript: (s: string) => {
      captured = s
      return Promise.resolve({ ok: true, detail: '' })
    }
  }
  return { wc, script: () => captured }
}

describe('browserActions page-world scripts', () => {
  it('clickRef never reads el.value (no sensitive-value leak into the label)', async () => {
    const m = mockWc()
    await clickRef(m.wc as never, '@e1', 1)
    expect(m.script()).not.toMatch(/el\.value/)
  })

  it('clickRef validates the element\'s own document generation (frame-aware)', async () => {
    const m = mockWc()
    await clickRef(m.wc as never, '@e1', 7)
    expect(m.script()).toContain('ownerDocument')
    expect(m.script()).toContain('biscuitFind')
  })

  it('typeRef reports only the typed length, not the typed text, in its detail', async () => {
    const m = mockWc()
    await typeRef(m.wc as never, '@e1', 'super-secret-value', 1)
    expect(m.script()).toContain("'typed '+value.length+' chars")
  })

  it('typeRef handles <select> by matching an option and refuses non-editable targets', async () => {
    const m = mockWc()
    await typeRef(m.wc as never, '@e1', 'x', 1)
    const s = m.script()
    expect(s).toContain("el.tagName === 'SELECT'")
    expect(s).toContain('is not an editable field')
  })
})
