/**
 * Newline-delimited JSON read off a streamed body, one record per line, as each line arrives: the shape of
 * `GET /v1/search/content` (ticket 59).
 */

/**
 * Every record in `stream`, in order, each as soon as its line is complete. A line split between chunks is
 * joined, and so is a character whose bytes are; blank lines are skipped and a last line without a newline
 * still counts. A line that is not JSON ends the reading with its `SyntaxError`.
 *
 * Aborting `signal` cancels the stream at once, even while it waits for bytes, and the reading fails with
 * the signal's reason. That, or leaving the loop early, closes a fetched body's connection: it is how a
 * content search is stopped.
 */
export async function* ndjsonRecords(
  stream: ReadableStream<Uint8Array>,
  signal?: AbortSignal,
): AsyncGenerator<unknown> {
  const reader = stream.getReader()
  const stop = () => void reader.cancel().catch(() => {})
  signal?.addEventListener('abort', stop, { once: true })
  const decoder = new TextDecoder()
  let pending = ''
  let finished = false
  try {
    for (;;) {
      signal?.throwIfAborted()
      const { done, value } = await reader.read()
      signal?.throwIfAborted()
      pending += done ? decoder.decode() : decoder.decode(value, { stream: true })
      const lines = pending.split('\n')
      pending = done ? '' : (lines.pop() ?? '')
      for (const line of lines) if (line.trim() !== '') yield JSON.parse(line) as unknown
      if (done) break
    }
    finished = true
  } finally {
    signal?.removeEventListener('abort', stop)
    if (!finished) await reader.cancel().catch(() => {})
    reader.releaseLock()
  }
}
