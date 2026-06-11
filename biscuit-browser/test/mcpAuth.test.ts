import { describe, it, expect } from 'vitest'
import type http from 'node:http'
import { authorizePost } from '../src/main/mcp/server'

const TOKEN = 'secret-token'
const PORT = 8765

function req(headers: Record<string, string | undefined>): http.IncomingMessage {
  return { headers } as unknown as http.IncomingMessage
}

const valid = {
  host: `127.0.0.1:${PORT}`,
  authorization: `Bearer ${TOKEN}`,
  'content-type': 'application/json'
}

describe('MCP server authorization', () => {
  it('accepts a well-formed authenticated local request', () => {
    expect(authorizePost(req(valid), TOKEN, PORT)).toBeNull()
  })

  it('rejects requests carrying a browser Origin (CSRF)', () => {
    expect(authorizePost(req({ ...valid, origin: 'https://evil.example' }), TOKEN, PORT)).toBe(
      'origin not allowed'
    )
  })

  it('rejects a wrong or missing bearer token', () => {
    expect(authorizePost(req({ ...valid, authorization: 'Bearer nope' }), TOKEN, PORT)).toBe(
      'unauthorized'
    )
    expect(authorizePost(req({ ...valid, authorization: undefined }), TOKEN, PORT)).toBe(
      'unauthorized'
    )
  })

  it('rejects a non-localhost Host (DNS rebinding)', () => {
    expect(authorizePost(req({ ...valid, host: 'attacker.example' }), TOKEN, PORT)).toBe(
      'invalid host'
    )
  })

  it('rejects the text/plain content-type used for no-preflight CSRF', () => {
    expect(authorizePost(req({ ...valid, 'content-type': 'text/plain' }), TOKEN, PORT)).toBe(
      'content-type must be application/json'
    )
  })
})
