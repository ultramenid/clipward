/** Bound decoded response bytes before buffering, including chunked bodies. */
export async function readResponseText(response: Response, maxBytes: number, tooLargeMessage = 'Provider response is too large'): Promise<string> {
  const chunks: Uint8Array[] = []
  let size = 0
  const reader = response.body?.getReader()
  if (!reader) return ''
  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      size += value.byteLength
      if (size > maxBytes) {
        await reader.cancel()
        throw new Error(tooLargeMessage)
      }
      chunks.push(value)
    }
  } finally { reader.releaseLock() }
  return Buffer.concat(chunks).toString('utf8')
}
