import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { homedir } from 'node:os'
import type { Client, Scope } from './prompts.js'

interface ClaudeSettings {
  hooks?: Record<string, unknown>
  [key: string]: unknown
}

export async function installHooks(client: Client, scope: Scope, url: string, key: string): Promise<string | null> {
  if (client !== 'claude-code') return null

  const dir = scope === 'project'
    ? join(process.cwd(), '.claude')
    : join(homedir(), '.claude')
  await mkdir(dir, { recursive: true })
  const filePath = join(dir, 'settings.json')

  const existing = await readJsonSafe<ClaudeSettings>(filePath)

  const inboxHook = {
    hooks: [
      {
        type: 'command' as const,
        command: `curl -sf "${url}/api/agents/me/inbox" -H "Authorization: Bearer ${key}" | head -c 500`,
      },
    ],
  }

  const config = {
    ...existing,
    hooks: {
      ...existing?.hooks,
      SessionStart: [inboxHook],
    },
  }

  await writeFile(filePath, JSON.stringify(config, null, 2) + '\n')
  return filePath
}

async function readJsonSafe<T>(path: string): Promise<T | undefined> {
  try {
    const content = await readFile(path, 'utf-8')
    return JSON.parse(content) as T
  } catch {
    return undefined
  }
}
