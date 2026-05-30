import { describe, it, expect } from 'vitest'
import { extractJson } from '../src/main/agent/llm'

describe('extractJson', () => {
  it('parses a clean JSON object', () => {
    expect(extractJson('{"a":1,"b":"x"}')).toEqual({ a: 1, b: 'x' })
  })

  it('parses a clean JSON array', () => {
    expect(extractJson('[1,2,3]')).toEqual([1, 2, 3])
  })

  it('strips ```json fences', () => {
    expect(extractJson('```json\n{"a":1}\n```')).toEqual({ a: 1 })
    expect(extractJson('```\n{"k":"v"}\n```')).toEqual({ k: 'v' })
  })

  it('extracts the first balanced object embedded in prose', () => {
    expect(extractJson('Sure! Here you go: {"kind":"done","message":"hi"} — done.')).toEqual({
      kind: 'done',
      message: 'hi'
    })
  })

  it('is string-aware (braces inside strings do not end the object)', () => {
    expect(extractJson('{"s":"a } b { c"}')).toEqual({ s: 'a } b { c' })
  })

  it('handles nested objects', () => {
    expect(extractJson('noise {"outer":{"inner":[1,{"x":2}]}} noise')).toEqual({
      outer: { inner: [1, { x: 2 }] }
    })
  })

  it('returns null when there is no JSON', () => {
    expect(extractJson('the model refused to answer')).toBeNull()
    expect(extractJson('')).toBeNull()
  })

  it('returns null for malformed JSON-looking text', () => {
    expect(extractJson('{not json at all')).toBeNull()
  })
})
