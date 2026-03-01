import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { dirname } from 'node:path'
import type { ConnectionMode } from '../prompts.js'

interface OpenClawConfig {
  plugins?: {
    allow?: string[]
    entries?: Record<string, unknown>
    [key: string]: unknown
  }
  [key: string]: unknown
}

export async function writeOpenClawConfig(
  url: string,
  key: string,
  configPath: string,
  mode: ConnectionMode,
  projectId?: string,
): Promise<string> {
  // Read existing config (merge-safe)
  const existing = await readJsonSafe<OpenClawConfig>(configPath)

  // Plugin-specific config nested under .config (required by OpenClaw)
  const pluginConfig: Record<string, unknown> = {
    url,
    apiKey: key,
    mode,
  }
  if (projectId) pluginConfig.projectId = projectId

  // Build allow list — ensure "opengate" is present
  const existingAllow: string[] = Array.isArray(existing?.plugins?.allow)
    ? existing.plugins.allow
    : []
  const allow = existingAllow.includes('openclaw')
    ? existingAllow
    : [...existingAllow, 'openclaw']

  const config: OpenClawConfig = {
    ...existing,
    // NOTE: do NOT inject agents.main or MCP server config here.
    // The OpenClaw plugin handles the OpenGate connection natively —
    // no MCP server is needed. Injecting agents.main causes OpenClaw
    // to reject the config with "Unrecognized key: main".
    plugins: {
      ...existing?.plugins,
      allow,
      entries: {
        ...existing?.plugins?.entries,
        // Correctly nested: OpenClaw expects { enabled, config: { ... } }
        openclaw: {
          enabled: true,
          config: pluginConfig,
        },
      },
    },
  }

  await mkdir(dirname(configPath), { recursive: true })
  await writeFile(configPath, JSON.stringify(config, null, 2) + '\n')
  return configPath
}

async function readJsonSafe<T>(path: string): Promise<T | undefined> {
  try {
    const content = await readFile(path, 'utf-8')
    return JSON.parse(content) as T
  } catch {
    return undefined
  }
}

/**
 * Write HEARTBEAT.md to the agent's OpenClaw workspace.
 * Instructs the agent to check both inbox (todo/in_progress) and
 * /api/tasks/mine (catches backlog tasks) on each heartbeat tick.
 */
export async function writeHeartbeatMd(
  url: string,
  key: string,
  workspacePath: string,
): Promise<string> {
  const content = `# HEARTBEAT.md

## Heartbeat — Check for Work

On each heartbeat, check OpenGate for assigned tasks:

\`\`\`bash
# Step 1: Check inbox (todo + in_progress tasks)
curl -s ${url}/api/agents/me/inbox \\
  -H "Authorization: Bearer ${key}"

# Step 2: If inbox is empty, check all my tasks (includes backlog)
curl -s ${url}/api/tasks/mine \\
  -H "Authorization: Bearer ${key}"
\`\`\`

- If \`todo\` or \`in_progress\` tasks exist in inbox → pick up the highest priority one and work on it
- If inbox is empty → check \`/api/tasks/mine\` for any \`backlog\` tasks with no unmet dependencies → claim and work on the highest priority one
- If nothing at all → do nothing (\`HEARTBEAT_OK\`)

## Notes

- \`/api/agents/me/inbox\` only returns \`todo\`/\`in_progress\` tasks — NOT backlog
- \`/api/tasks/mine\` returns ALL tasks assigned to you regardless of status

## My Identity on OpenGate

- **URL:** ${url}
- **API Key:** \`${key}\`
`

  await mkdir(workspacePath, { recursive: true })
  const filePath = `${workspacePath}/HEARTBEAT.md`
  await writeFile(filePath, content)
  return filePath
}
