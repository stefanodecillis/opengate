#!/usr/bin/env node
import * as p from '@clack/prompts'
import { execFileSync } from 'node:child_process'
import { validateConnection, sendHeartbeat } from './api.js'
import { askClient, askScope, askProjectId, askOpenClawConfigPath, askConnectionMode, askWorkspacePath } from './prompts.js'
import { writeClaudeCodeConfig } from './clients/claude-code.js'
import { writeOpenCodeConfig } from './clients/opencode.js'
import { writeOpenClawConfig, writeHeartbeatMd } from './clients/openclaw.js'
import { installSkill } from './skill.js'
import { installHooks } from './hooks.js'

function parseArgs(argv: string[]): { url?: string; key?: string } {
  const args: Record<string, string> = {}
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--url' && argv[i + 1]) args.url = argv[++i]
    else if (argv[i] === '--key' && argv[i + 1]) args.key = argv[++i]
    else if (argv[i]?.startsWith('--url=')) args.url = argv[i].slice(6)
    else if (argv[i]?.startsWith('--key=')) args.key = argv[i].slice(6)
  }
  return args
}

async function main() {
  const { url, key } = parseArgs(process.argv.slice(2))

  p.intro('OpenGate Agent Setup')

  if (!url || !key) {
    p.cancel('Missing required arguments: --url <url> --key <api-key>')
    process.exit(1)
  }

  // 1. Validate connection
  const validating = p.spinner()
  validating.start('Validating connection…')
  try {
    const me = await validateConnection(url, key)
    validating.stop(`Connected as "${me.name}"`)
  } catch (err) {
    validating.stop('Connection failed')
    p.log.error(err instanceof Error ? err.message : String(err))
    process.exit(1)
  }

  // 2. Ask which client
  const client = await askClient()

  if (client === 'other') {
    p.log.info('Manual setup: configure your MCP client to connect to:')
    p.log.info(`  URL: ${url}`)
    p.log.info(`  Key: ${key}`)
    p.log.info('See https://opengate.sh/docs/setup for details.')

    const hb = p.spinner()
    hb.start('Sending heartbeat…')
    try {
      await sendHeartbeat(url, key)
      hb.stop('Heartbeat sent')
    } catch {
      hb.stop('Heartbeat failed (you can retry later)')
    }

    p.outro('Done! Configure your client manually to complete setup.')
    return
  }

  // OpenClaw has its own flow
  if (client === 'openclaw') {
    const configPath = await askOpenClawConfigPath()
    const mode = await askConnectionMode()
    const scopedProjectId = await askProjectId()
    const workspacePath = await askWorkspacePath()

    const configSpinner = p.spinner()
    configSpinner.start('Writing openclaw.json…')
    try {
      const written = await writeOpenClawConfig(url, key, configPath, mode, scopedProjectId)
      configSpinner.stop(`Config written → ${written}`)
    } catch (err) {
      configSpinner.stop('Failed to write config')
      p.log.error(err instanceof Error ? err.message : String(err))
      process.exit(1)
    }

    const skillSpinner = p.spinner()
    skillSpinner.start('Installing skill…')
    try {
      const skillPath = await installSkill('openclaw', 'global')
      skillSpinner.stop(skillPath ? `Skill installed → ${skillPath}` : 'Skill skipped')
    } catch (err) {
      skillSpinner.stop('Skill installation failed (non-critical)')
      p.log.warn(err instanceof Error ? err.message : String(err))
    }

    if (workspacePath) {
      const hbMdSpinner = p.spinner()
      hbMdSpinner.start('Writing HEARTBEAT.md…')
      try {
        const hbMdPath = await writeHeartbeatMd(url, key, workspacePath)
        hbMdSpinner.stop(`HEARTBEAT.md written → ${hbMdPath}`)
      } catch (err) {
        hbMdSpinner.stop('HEARTBEAT.md failed (non-critical)')
        p.log.warn(err instanceof Error ? err.message : String(err))
      }
    }

    const hbSpinner = p.spinner()
    hbSpinner.start('Sending heartbeat…')
    try {
      await sendHeartbeat(url, key)
      hbSpinner.stop('Heartbeat sent — you\'re connected!')
    } catch {
      hbSpinner.stop('Heartbeat failed (will retry automatically)')
    }

    const pluginSpinner = p.spinner()
    pluginSpinner.start('Installing opengate plugin (latest)…')
    try {
      execFileSync('openclaw', ['plugins', 'install', 'opengate@latest'], { stdio: 'pipe' })
      pluginSpinner.stop('Plugin installed (latest)')
    } catch {
      pluginSpinner.stop('Plugin auto-install failed')
      p.log.warn('Run manually: openclaw plugins install opengate@latest')
    }
    p.outro('Setup complete! Your agent is ready to receive tasks.')
    return
  }

  // 3. Ask scope (project vs global)
  const scope = await askScope()

  // 3b. Ask for project scoping (optional)
  let scopedProjectId: string | undefined
  if (scope === 'project') {
    scopedProjectId = await askProjectId()
  }

  // 4. Write MCP config
  const configSpinner = p.spinner()
  configSpinner.start('Configuring MCP server…')
  try {
    let configPath: string
    if (client === 'claude-code') {
      configPath = await writeClaudeCodeConfig(url, key, scope, scopedProjectId)
    } else {
      configPath = await writeOpenCodeConfig(url, key, scopedProjectId)
    }
    configSpinner.stop(`MCP server configured → ${configPath}`)
  } catch (err) {
    configSpinner.stop('Failed to write MCP config')
    p.log.error(err instanceof Error ? err.message : String(err))
    process.exit(1)
  }

  // 5. Install skill
  const skillSpinner = p.spinner()
  skillSpinner.start('Installing skill…')
  try {
    const skillPath = await installSkill(client, scope)
    if (skillPath) {
      skillSpinner.stop(`Skill installed → ${skillPath}`)
    } else {
      skillSpinner.stop('Skill installation skipped')
    }
  } catch (err) {
    skillSpinner.stop('Skill installation failed (non-critical)')
    p.log.warn(err instanceof Error ? err.message : String(err))
  }

  // 6. Install hooks
  const hookSpinner = p.spinner()
  hookSpinner.start('Configuring hooks…')
  try {
    const hookPath = await installHooks(client, scope, url, key)
    if (hookPath) {
      hookSpinner.stop(`Hooks configured → ${hookPath}`)
    } else {
      hookSpinner.stop('Hooks skipped (not applicable for this client)')
    }
  } catch (err) {
    hookSpinner.stop('Hook configuration failed (non-critical)')
    p.log.warn(err instanceof Error ? err.message : String(err))
  }

  // 7. Send heartbeat
  const hbSpinner = p.spinner()
  hbSpinner.start('Sending heartbeat…')
  try {
    await sendHeartbeat(url, key)
    hbSpinner.stop('Heartbeat sent — you\'re connected!')
  } catch {
    hbSpinner.stop('Heartbeat failed (the dashboard will retry)')
  }

  p.outro('Setup complete! Your agent is ready to receive tasks.')
}

main().catch((err) => {
  console.error('Unexpected error:', err)
  process.exit(1)
})
