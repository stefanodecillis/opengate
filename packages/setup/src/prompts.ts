import * as p from '@clack/prompts'

export type Client = 'claude-code' | 'opencode' | 'openclaw' | 'other'
export type ConnectionMode = 'polling' | 'websocket'
export type Scope = 'project' | 'global'

export async function askClient(): Promise<Client> {
  const result = await p.select({
    message: 'Which client are you using?',
    options: [
      { value: 'claude-code' as const, label: 'Claude Code' },
      { value: 'opencode' as const, label: 'OpenCode' },
      { value: 'openclaw' as const, label: 'OpenClaw' },
      { value: 'other' as const, label: 'Other (manual config)' },
    ],
  })
  if (p.isCancel(result)) {
    p.cancel('Setup cancelled.')
    process.exit(0)
  }
  return result
}

export async function askScope(): Promise<Scope> {
  const result = await p.select({
    message: 'Installation scope:',
    options: [
      { value: 'project' as const, label: 'This project only (.mcp.json)' },
      { value: 'global' as const, label: 'Global (~/.claude/settings.json)' },
    ],
  })
  if (p.isCancel(result)) {
    p.cancel('Setup cancelled.')
    process.exit(0)
  }
  return result
}

export async function askProjectId(): Promise<string | undefined> {
  const result = await p.text({
    message: 'Scope to a specific project ID? (leave blank for all projects)',
    placeholder: 'e.g. proj_abc123',
  })
  if (p.isCancel(result)) {
    p.cancel('Setup cancelled.')
    process.exit(0)
  }
  return result || undefined
}

export async function askOpenClawConfigPath(): Promise<string> {
  const defaultPath = `${process.env.HOME ?? '~'}/.openclaw/openclaw.json`
  const result = await p.text({
    message: 'OpenClaw config path:',
    placeholder: defaultPath,
    defaultValue: defaultPath,
  })
  if (p.isCancel(result)) {
    p.cancel('Setup cancelled.')
    process.exit(0)
  }
  return (result as string) || defaultPath
}

export async function askWorkspacePath(): Promise<string | null> {
  const result = await p.text({
    message: 'Agent workspace path for HEARTBEAT.md? (leave blank to skip)',
    placeholder: `${process.env.HOME ?? '~'}/.openclaw/workspace-myagent`,
  })
  if (p.isCancel(result)) {
    p.cancel('Setup cancelled.')
    process.exit(0)
  }
  return (result as string) || null
}

export async function askConnectionMode(): Promise<ConnectionMode> {
  const result = await p.select({
    message: 'Notification mode:',
    options: [
      { value: 'polling' as const, label: 'Polling (default, no persistent connection)' },
      { value: 'websocket' as const, label: 'WebSocket (real-time, persistent connection)' },
    ],
  })
  if (p.isCancel(result)) {
    p.cancel('Setup cancelled.')
    process.exit(0)
  }
  return result
}
