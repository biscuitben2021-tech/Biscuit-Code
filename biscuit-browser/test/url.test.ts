import { describe, it, expect } from 'vitest'
import { normalizeUrl, searchUrl } from '@shared/url'

const SEARCH = 'https://duckduckgo.com/?q='

describe('normalizeUrl', () => {
  it('passes through absolute http(s) URLs unchanged', () => {
    expect(normalizeUrl('https://example.com')).toBe('https://example.com')
    expect(normalizeUrl('http://example.com/path?q=1')).toBe('http://example.com/path?q=1')
  })

  it('matches the scheme case-insensitively', () => {
    expect(normalizeUrl('HTTPS://Example.com')).toBe('HTTPS://Example.com')
  })

  it('trims surrounding whitespace', () => {
    expect(normalizeUrl('  https://example.com  ')).toBe('https://example.com')
  })

  it('upgrades schemeless domain-like input to https', () => {
    expect(normalizeUrl('example.com')).toBe('https://example.com')
    expect(normalizeUrl('example.com/path?q=1')).toBe('https://example.com/path?q=1')
    expect(normalizeUrl('127.0.0.1')).toBe('https://127.0.0.1')
  })

  it('keeps about:blank navigable', () => {
    expect(normalizeUrl('about:blank')).toBe('about:blank')
    expect(normalizeUrl('ABOUT:BLANK')).toBe('about:blank')
  })

  it('routes filesystem/script schemes to a web search (never loads them)', () => {
    for (const dangerous of [
      'file:///etc/passwd',
      'javascript:alert(1)',
      'data:text/html,<script>1</script>',
      'blob:https://x/y',
      'chrome://settings',
      'view-source:http://x',
      'about:config'
    ]) {
      const out = normalizeUrl(dangerous)
      expect(out.startsWith(SEARCH)).toBe(true)
      expect(out.startsWith('file:')).toBe(false)
      expect(out.startsWith('javascript:')).toBe(false)
    }
  })

  it('treats free text / multi-word input as a search query', () => {
    expect(normalizeUrl('hello world')).toBe(SEARCH + encodeURIComponent('hello world'))
    expect(normalizeUrl('latest rust release')).toBe(SEARCH + encodeURIComponent('latest rust release'))
    // No dot / TLD → not a domain.
    expect(normalizeUrl('localhost')).toBe(SEARCH + encodeURIComponent('localhost'))
  })

  it('handles empty input without throwing', () => {
    expect(normalizeUrl('')).toBe(SEARCH + '')
    expect(normalizeUrl('   ')).toBe(SEARCH + '')
  })
})

describe('searchUrl', () => {
  it('percent-encodes the query', () => {
    expect(searchUrl('a b&c')).toBe(SEARCH + encodeURIComponent('a b&c'))
  })
})
