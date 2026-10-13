import { describe, expect, it } from 'vitest'
import { ndjsonRecords } from './ndjson'

/** A byte stream handing out `chunks` one read at a time, as a network body does. */
const body = (...chunks: (string | Uint8Array)[]) =>
  new ReadableStream<Uint8Array>({
    start(controller) {
      for (const chunk of chunks)
        controller.enqueue(typeof chunk === 'string' ? new TextEncoder().encode(chunk) : chunk)
      controller.close()
    },
  })

const all = async (stream: ReadableStream<Uint8Array>) => {
  const records: unknown[] = []
  for await (const record of ndjsonRecords(stream)) records.push(record)
  return records
}

describe('reading newline-delimited JSON', () => {
  it('reads one record per line', async () => {
    expect(await all(body('{"a":1}\n{"b":2}\n'))).toEqual([{ a: 1 }, { b: 2 }])
  })

  it('joins a line split between chunks', async () => {
    expect(await all(body('{"type":"ma', 'tch","line":3}\n{"x"', ':1}\n'))).toEqual([
      { type: 'match', line: 3 },
      { x: 1 },
    ])
  })

  it('joins a character whose bytes are split between chunks', async () => {
    const bytes = new TextEncoder().encode('{"s":"é"}\n')
    const at = bytes.indexOf(0xc3) + 1

    expect(await all(body(bytes.slice(0, at), bytes.slice(at)))).toEqual([{ s: 'é' }])
  })

  it('skips empty lines', async () => {
    expect(await all(body('\n{"a":1}\n\n\r\n{"b":2}\n'))).toEqual([{ a: 1 }, { b: 2 }])
  })

  it('reads a last line that has no newline', async () => {
    expect(await all(body('{"a":1}\n{"b":2}'))).toEqual([{ a: 1 }, { b: 2 }])
  })

  it('reads nothing from an empty body', async () => {
    expect(await all(body())).toEqual([])
  })

  it('fails on a line that is not JSON', async () => {
    await expect(all(body('{"a":1}\nnot json\n'))).rejects.toBeInstanceOf(SyntaxError)
  })

  it('stops at once when its signal is aborted, failing with the reason', async () => {
    let writer!: ReadableStreamDefaultController<Uint8Array>
    const open = new ReadableStream<Uint8Array>({ start: (c) => void (writer = c) })
    const controller = new AbortController()
    writer.enqueue(new TextEncoder().encode('{"a":1}\n'))

    const records: unknown[] = []
    const reading = (async () => {
      for await (const record of ndjsonRecords(open, controller.signal)) records.push(record)
    })()
    await new Promise((resolve) => setTimeout(resolve, 0))
    controller.abort(new Error('cancelled'))

    await expect(reading).rejects.toThrow('cancelled')
    expect(records).toEqual([{ a: 1 }])
  })
})
