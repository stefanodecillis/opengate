import { writeFile, mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { homedir } from 'node:os'
import type { Scope } from '../prompts.js'
import { readJsonSafe } from '../read-json.js'

interface McpConfig {
  mcpServers?: Record<string, unknown>
  [key: string]: unknown
}

export async function writeClaudeCodeConfig(url: string, key: string, scope: Scope, projectId?: string): Promise<string> {
  const env: Record<string, string> = {
    OPENGATE_URL: url,
    OPENGATE_API_KEY: key,
  }
  if (projectId) env.OPENGATE_PROJECT_ID = projectId

  const mcpEntry = {
    command: 'npx',
    args: ['-y', '@opengate/mcp@latest'],
    env,
  }

  if (scope === 'project') {
    return writeProjectConfig(mcpEntry)
  }
  return writeGlobalConfig(mcpEntry)
}

async function writeProjectConfig(mcpEntry: unknown): Promise<string> {
  const filePath = join(process.cwd(), '.mcp.json')
  const existing = await readJsonSafe<McpConfig>(filePath)
  const config = {
    ...existing,
    mcpServers: {
      ...existing?.mcpServers,
      opengate: mcpEntry,
    },
  }
  await writeFile(filePath, JSON.stringify(config, null, 2) + '\n')
  return filePath
}

async function writeGlobalConfig(mcpEntry: unknown): Promise<string> {
  const dir = join(homedir(), '.claude')
  await mkdir(dir, { recursive: true })
  const filePath = join(dir, 'settings.json')
  const existing = await readJsonSafe<McpConfig>(filePath)
  const config = {
    ...existing,
    mcpServers: {
      ...existing?.mcpServers,
      opengate: mcpEntry,
    },
  }
  await writeFile(filePath, JSON.stringify(config, null, 2) + '\n')
  return filePath
}
