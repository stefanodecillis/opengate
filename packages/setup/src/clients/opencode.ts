import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { homedir } from 'node:os'

interface OpenCodeConfig {
  mcp?: Record<string, unknown>
  [key: string]: unknown
}

export async function writeOpenCodeConfig(url: string, key: string, projectId?: string): Promise<string> {
  const dir = join(homedir(), '.config', 'opencode')
  await mkdir(dir, { recursive: true })
  const filePath = join(dir, 'opencode.json')

  const environment: Record<string, string> = {
    OPENGATE_URL: url,
    OPENGATE_API_KEY: key,
  }
  if (projectId) environment.OPENGATE_PROJECT_ID = projectId

  const existing = await readJsonSafe<OpenCodeConfig>(filePath)
  const config = {
    ...existing,
    mcp: {
      ...existing?.mcp,
      opengate: {
        type: 'local',
        command: ['npx', '-y', '@opengate/mcp@latest'],
        enabled: true,
        environment,
      },
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
