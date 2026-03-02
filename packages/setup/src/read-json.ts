import { readFile } from 'node:fs/promises'

/**
 * Read and parse a JSON file safely.
 * - File not found (ENOENT) → returns undefined (expected on first run)
 * - Parse error → throws with actionable message (prevents silent config wipe)
 */
export async function readJsonSafe<T>(path: string): Promise<T | undefined> {
  let content: string
  try {
    content = await readFile(path, 'utf-8')
  } catch (err: unknown) {
    if ((err as NodeJS.ErrnoException).code === 'ENOENT') return undefined
    throw err
  }
  try {
    return JSON.parse(content) as T
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err)
    throw new Error(`Cannot parse ${path}: ${detail}. Fix the file or delete it to start fresh.`)
  }
}
