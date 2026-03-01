#!/usr/bin/env node
import { createInterface } from 'node:readline'

// ── Version ─────────────────────────────────────

const PKG_VERSION = '0.1.1'

// ── Config ──────────────────────────────────────

const url = process.env.OPENGATE_URL || parseArg('--url')
const key = process.env.OPENGATE_API_KEY || parseArg('--key')
const projectId = process.env.OPENGATE_PROJECT_ID || parseArg('--project-id')

if (!url || !key) {
  process.stderr.write(
    'Usage: opengate-mcp --url <server-url> --key <api-key>\n' +
    '  or set OPENGATE_URL and OPENGATE_API_KEY env vars\n'
  )
  process.exit(1)
}

const baseUrl = url.replace(/\/+$/, '')
const headers = {
  'Authorization': `Bearer ${key}`,
  'Content-Type': 'application/json',
}

// ── HTTP helpers ────────────────────────────────

async function get(path: string): Promise<unknown> {
  const res = await fetch(`${baseUrl}${path}`, { headers })
  if (!res.ok) {
    const text = await res.text().catch(() => '')
    throw new Error(`${res.status}: ${text || res.statusText}`)
  }
  return res.json()
}

async function post(path: string, body?: unknown): Promise<unknown> {
  const res = await fetch(`${baseUrl}${path}`, {
    method: 'POST',
    headers,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
  if (!res.ok) {
    const text = await res.text().catch(() => '')
    throw new Error(`${res.status}: ${text || res.statusText}`)
  }
  const contentType = res.headers.get('content-type') || ''
  if (contentType.includes('application/json')) return res.json()
  return { ok: true }
}

async function patch(path: string, body: unknown): Promise<unknown> {
  const res = await fetch(`${baseUrl}${path}`, {
    method: 'PATCH',
    headers,
    body: JSON.stringify(body),
  })
  if (!res.ok) {
    const text = await res.text().catch(() => '')
    throw new Error(`${res.status}: ${text || res.statusText}`)
  }
  return res.json()
}

async function put(path: string, body: unknown): Promise<unknown> {
  const res = await fetch(`${baseUrl}${path}`, {
    method: 'PUT',
    headers,
    body: JSON.stringify(body),
  })
  if (!res.ok) {
    const text = await res.text().catch(() => '')
    throw new Error(`${res.status}: ${text || res.statusText}`)
  }
  return res.json()
}

async function del(path: string): Promise<number> {
  const res = await fetch(`${baseUrl}${path}`, {
    method: 'DELETE',
    headers: { Authorization: `Bearer ${key}` },
  })
  if (!res.ok) {
    const text = await res.text().catch(() => '')
    throw new Error(`${res.status}: ${text || res.statusText}`)
  }
  return res.status
}

// ── Tool definitions (mirrors Rust mcp.rs) ──────

const TOOLS = [
  tool('check_inbox', 'Check your work inbox. Returns assigned tasks, open questions, unread notifications. Call this FIRST.', {
    project_id: { type: 'string', description: 'Filter by project ID' },
  }),
  tool('list_projects', 'List all projects', {
    status: { type: 'string', description: 'Filter by status: active or archived' },
  }),
  tool('get_project', 'Get project details with task summary', {
    id: { type: 'string', description: 'Project ID' },
  }, ['id']),
  tool('create_project', 'Create a new project', {
    name: { type: 'string', description: 'Project name' },
    description: { type: 'string', description: 'Project description' },
    repo_url: { type: 'string', description: 'Git repository URL' },
    default_branch: { type: 'string', description: 'Default branch name (e.g. main)' },
  }, ['name']),
  tool('update_project', 'Update project fields', {
    id: { type: 'string', description: 'Project ID' },
    name: { type: 'string', description: 'Project name' },
    description: { type: 'string', description: 'Project description' },
    repo_url: { type: 'string', description: 'Git repository URL' },
    default_branch: { type: 'string', description: 'Default branch name' },
    status: { type: 'string', description: 'Project status: active or archived' },
  }, ['id']),
  tool('get_workspace_info', 'Get workspace info for a project (repo URL, branch, suggested local path)', {
    project_id: { type: 'string', description: 'Project ID' },
  }, ['project_id']),
  tool('list_tasks', 'List tasks with optional filters', {
    project_id: { type: 'string', description: 'Filter by project ID' },
    status: { type: 'string', description: 'Filter by status' },
    priority: { type: 'string', description: 'Filter by priority' },
    assignee_id: { type: 'string', description: 'Filter by assignee ID' },
    tag: { type: 'string', description: 'Filter by tag' },
  }),
  tool('get_task', 'Get task with full context and output', {
    id: { type: 'string', description: 'Task ID' },
  }, ['id']),
  tool('create_task', 'Create a new task in a project', {
    project_id: { type: 'string', description: 'Project ID' },
    title: { type: 'string', description: 'Task title' },
    description: { type: 'string', description: 'Task description' },
    priority: { type: 'string', description: 'Priority: critical, high, medium, low' },
    tags: { type: 'array', items: { type: 'string' }, description: 'Skill/category tags' },
    context: { type: 'object', description: 'Structured context' },
    due_date: { type: 'string', description: 'Due date (ISO 8601)' },
  }, ['project_id', 'title']),
  tool('update_task', 'Update task fields', {
    id: { type: 'string', description: 'Task ID' },
    title: { type: 'string' },
    description: { type: 'string' },
    status: { type: 'string' },
    priority: { type: 'string' },
    tags: { type: 'array', items: { type: 'string' } },
    context: { type: 'object' },
    output: { type: 'object' },
    due_date: { type: 'string' },
  }, ['id']),
  tool('claim_task', 'Claim an unassigned task', {
    id: { type: 'string', description: 'Task ID' },
  }, ['id']),
  tool('release_task', 'Release a claimed task back to pool', {
    id: { type: 'string', description: 'Task ID' },
  }, ['id']),
  tool('complete_task', 'Mark task as done with optional summary and output', {
    id: { type: 'string', description: 'Task ID' },
    summary: { type: 'string', description: 'Completion summary' },
    output: { type: 'object', description: 'Deliverables: PR URLs, file paths, artifacts' },
  }, ['id']),
  tool('block_task', 'Mark task as blocked', {
    id: { type: 'string', description: 'Task ID' },
    reason: { type: 'string', description: 'Reason for blocking' },
  }, ['id']),
  tool('next_task', 'Find highest priority unclaimed task matching skills', {
    skills: { type: 'array', items: { type: 'string' }, description: 'Skills to match' },
    project_id: { type: 'string', description: 'Filter by project ID' },
  }),
  tool('my_tasks', 'List all tasks assigned to you', {
    project_id: { type: 'string', description: 'Filter by project ID' },
  }),
  tool('update_context', 'Merge-patch task context', {
    id: { type: 'string', description: 'Task ID' },
    context_patch: { type: 'object', description: 'Fields to merge into existing context' },
  }, ['id', 'context_patch']),
  tool('post_comment', 'Post a comment on a task', {
    task_id: { type: 'string', description: 'Task ID' },
    content: { type: 'string', description: 'Comment content' },
  }, ['task_id', 'content']),
  tool('heartbeat', 'Report agent liveness', {}),
  tool('list_agents', 'List all registered agents', {}),
  tool('get_agent_profile', 'Get full agent profile', {
    agent_id: { type: 'string', description: 'Agent ID' },
  }, ['agent_id']),
  tool('update_agent_profile', 'Update this agent\'s profile', {
    description: { type: 'string', description: 'Agent description' },
    skills: { type: 'array', items: { type: 'string' }, description: 'Agent skills (task-matching keywords)' },
    max_concurrent_tasks: { type: 'integer', description: 'Max concurrent tasks' },
    webhook_url: { type: 'string', description: 'Webhook URL for notifications' },
    webhook_events: { type: 'array', items: { type: 'string' }, description: 'Event types to subscribe to (null = all)' },
    config: { type: 'object', description: 'Arbitrary agent config JSON' },
    model: { type: 'string', description: 'LLM model identifier' },
    provider: { type: 'string', description: 'LLM provider' },
    cost_tier: { type: 'string', description: 'Cost tier (free|standard|premium)' },
    capabilities: { type: 'array', items: { type: 'string' }, description: 'Capability strings (e.g. code-review:rust)' },
    seniority: { type: 'string', description: 'Agent seniority: junior | mid | senior' },
    role: { type: 'string', description: 'Agent role: executor | orchestrator' },
    stale_timeout: { type: 'integer', description: 'Minutes before considered stale (default: 240)' },
    tags: { type: 'array', items: { type: 'string' }, description: 'Category tags (e.g. ["rust", "frontend", "devops"])' },
  }),
  tool('assign_task', 'Assign a task to a specific agent', {
    task_id: { type: 'string', description: 'Task ID' },
    agent_id: { type: 'string', description: 'Agent ID to assign to' },
  }, ['task_id', 'agent_id']),
  tool('handoff_task', 'Hand off a task to another agent', {
    task_id: { type: 'string', description: 'Task ID' },
    to_agent_id: { type: 'string', description: 'Target agent ID' },
    summary: { type: 'string', description: 'Handoff summary' },
  }, ['task_id', 'to_agent_id']),
  tool('approve_task', 'Approve a task in review', {
    task_id: { type: 'string', description: 'Task ID' },
    comment: { type: 'string', description: 'Approval comment' },
  }, ['task_id']),
  tool('request_changes', 'Request changes on a task in review', {
    task_id: { type: 'string', description: 'Task ID' },
    comment: { type: 'string', description: 'What changes are needed' },
  }, ['task_id', 'comment']),
  tool('get_knowledge', 'Get a knowledge base entry by key', {
    project_id: { type: 'string', description: 'Project ID' },
    key: { type: 'string', description: 'Knowledge entry key' },
  }, ['project_id', 'key']),
  tool('set_knowledge', 'Create or update a knowledge base entry', {
    project_id: { type: 'string', description: 'Project ID' },
    key: { type: 'string', description: 'Knowledge entry key' },
    title: { type: 'string', description: 'Entry title' },
    content: { type: 'string', description: 'Entry content (markdown)' },
    tags: { type: 'array', items: { type: 'string' }, description: 'Tags' },
    category: { type: 'string', enum: ['architecture', 'pattern', 'gotcha', 'decision', 'reference'], description: 'Category' },
  }, ['project_id', 'key', 'title', 'content']),
  tool('search_knowledge', 'Search project knowledge base', {
    project_id: { type: 'string', description: 'Project ID' },
    query: { type: 'string', description: 'Text search query' },
    tags: { type: 'array', items: { type: 'string' }, description: 'Filter by tags' },
    category: { type: 'string', description: 'Filter by category' },
  }, ['project_id']),
  tool('list_knowledge', 'List knowledge base entries for a project', {
    project_id: { type: 'string', description: 'Project ID' },
    prefix: { type: 'string', description: 'Filter by key prefix' },
  }, ['project_id']),
  tool('get_notifications', 'Get your notifications', {
    unread_only: { type: 'boolean', description: 'Only unread (default: true)' },
  }),
  tool('ack_notification', 'Mark notification(s) as read. Omit id to ack all.', {
    id: { type: 'integer', description: 'Notification ID (omit to ack all)' },
  }),
  tool('add_dependencies', 'Add dependency links to a task (blocks until dependencies complete)', {
    task_id: { type: 'string', description: 'Task to add dependencies to' },
    depends_on: { type: 'array', items: { type: 'string' }, description: 'Array of task IDs this task depends on' },
  }, ['task_id', 'depends_on']),
  tool('remove_dependency', 'Remove a dependency link from a task', {
    task_id: { type: 'string', description: 'Task to remove dependency from' },
    dependency_id: { type: 'string', description: 'ID of the dependency task to unlink' },
  }, ['task_id', 'dependency_id']),
  tool('list_dependencies', 'List tasks that a task depends on (must complete before it)', {
    task_id: { type: 'string', description: 'Task ID to list dependencies for' },
  }, ['task_id']),
  tool('list_dependents', 'List tasks that depend on a task (blocked by it)', {
    task_id: { type: 'string', description: 'Task ID to list dependents for' },
  }, ['task_id']),
]

// ── Project scoping ─────────────────────────────

const PROJECT_SCOPED_TOOLS = new Set([
  'list_tasks', 'create_task', 'next_task', 'check_inbox', 'my_tasks',
  'list_knowledge', 'search_knowledge', 'get_knowledge', 'set_knowledge',
  'get_workspace_info',
])

// ── Tool dispatch ───────────────────────────────

async function dispatch(name: string, args: Record<string, unknown>): Promise<unknown> {
  switch (name) {
    // Inbox & agents
    case 'check_inbox': {
      const qs = args.project_id ? `?project_id=${args.project_id}` : ''
      return get(`/api/agents/me/inbox${qs}`)
    }
    case 'heartbeat':
      return post('/api/agents/heartbeat')
    case 'list_agents':
      return get('/api/agents')
    case 'get_agent_profile':
      return get(`/api/agents/${args.agent_id}`)
    case 'update_agent_profile':
      return patch('/api/agents/me', args)
    case 'get_notifications': {
      const unread = args.unread_only !== false ? 'true' : 'false'
      return get(`/api/agents/me/notifications?unread_only=${unread}`)
    }
    case 'ack_notification':
      if (args.id != null) return post(`/api/agents/me/notifications/${args.id}/ack`)
      return post('/api/agents/me/notifications/ack-all')

    // Projects
    case 'list_projects': {
      const qs = args.status ? `?status=${args.status}` : ''
      return get(`/api/projects${qs}`)
    }
    case 'get_project':
      return get(`/api/projects/${args.id}`)
    case 'create_project':
      return post('/api/projects', {
        name: args.name,
        description: args.description,
        repo_url: args.repo_url,
        default_branch: args.default_branch,
      })
    case 'update_project': {
      const { id, ...body } = args
      return patch(`/api/projects/${id}`, body)
    }
    case 'get_workspace_info': {
      const project = await get(`/api/projects/${args.project_id}`) as { project?: { name?: string; repo_url?: string; default_branch?: string } }
      const p = project.project ?? project as { name?: string; repo_url?: string; default_branch?: string }
      const name = p.name ?? 'unknown'
      const slug = name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '')
      return {
        repo_url: p.repo_url ?? null,
        default_branch: p.default_branch ?? null,
        project_name: name,
        suggested_workspace_path: `~/.opengate/workspaces/${slug}`,
      }
    }

    // Tasks
    case 'list_tasks': {
      const params = new URLSearchParams()
      if (args.project_id) params.set('project_id', String(args.project_id))
      if (args.status) params.set('status', String(args.status))
      if (args.priority) params.set('priority', String(args.priority))
      if (args.assignee_id) params.set('assignee_id', String(args.assignee_id))
      if (args.tag) params.set('tag', String(args.tag))
      const qs = params.toString()
      return get(`/api/tasks${qs ? `?${qs}` : ''}`)
    }
    case 'get_task':
      return get(`/api/tasks/${args.id}`)
    case 'create_task': {
      const { project_id, ...body } = args
      return post(`/api/projects/${project_id}/tasks`, body)
    }
    case 'update_task': {
      const { id, ...body } = args
      return patch(`/api/tasks/${id}`, body)
    }
    case 'claim_task':
      return post(`/api/tasks/${args.id}/claim`)
    case 'release_task':
      return post(`/api/tasks/${args.id}/release`)
    case 'complete_task':
      return post(`/api/tasks/${args.id}/complete`, {
        summary: args.summary,
        output: args.output,
      })
    case 'block_task':
      return post(`/api/tasks/${args.id}/block`, { reason: args.reason })
    case 'next_task': {
      const params = new URLSearchParams()
      if (Array.isArray(args.skills) && args.skills.length) params.set('skills', args.skills.join(','))
      if (args.project_id) params.set('project_id', String(args.project_id))
      const qs = params.toString()
      return get(`/api/tasks/next${qs ? `?${qs}` : ''}`)
    }
    case 'my_tasks': {
      const qs = args.project_id ? `?project_id=${args.project_id}` : ''
      return get(`/api/tasks/mine${qs}`)
    }
    case 'update_context': {
      return patch(`/api/tasks/${args.id}/context`, args.context_patch)
    }
    case 'post_comment':
      return post(`/api/tasks/${args.task_id}/activity`, {
        content: args.content,
        activity_type: 'comment',
      })

    // Assignment & review
    case 'assign_task':
      return post(`/api/tasks/${args.task_id}/assign`, { agent_id: args.agent_id })
    case 'handoff_task':
      return post(`/api/tasks/${args.task_id}/handoff`, {
        to_agent_id: args.to_agent_id,
        summary: args.summary,
      })
    case 'approve_task':
      return post(`/api/tasks/${args.task_id}/approve`, { comment: args.comment })
    case 'request_changes':
      return post(`/api/tasks/${args.task_id}/request-changes`, { comment: args.comment })

    // Dependencies
    case 'add_dependencies':
      return post(`/api/tasks/${args.task_id}/dependencies`, { depends_on: args.depends_on })
    case 'remove_dependency': {
      await del(`/api/tasks/${args.task_id}/dependencies/${args.dependency_id}`)
      return { success: true, message: `Dependency ${args.dependency_id} removed from task ${args.task_id}` }
    }
    case 'list_dependencies':
      return get(`/api/tasks/${args.task_id}/dependencies`)
    case 'list_dependents':
      return get(`/api/tasks/${args.task_id}/dependents`)

    // Knowledge
    case 'get_knowledge':
      return get(`/api/projects/${args.project_id}/knowledge/${args.key}`)
    case 'set_knowledge': {
      const { project_id, key: k, ...body } = args
      return put(`/api/projects/${project_id}/knowledge/${k}`, body)
    }
    case 'search_knowledge': {
      const sp = new URLSearchParams()
      if (args.query) sp.set('q', String(args.query))
      if (args.category) sp.set('category', String(args.category))
      const qs = sp.toString()
      return get(`/api/projects/${args.project_id}/knowledge/search${qs ? `?${qs}` : ''}`)
    }
    case 'list_knowledge': {
      const qs = args.prefix ? `?prefix=${args.prefix}` : ''
      return get(`/api/projects/${args.project_id}/knowledge${qs}`)
    }

    default:
      throw new Error(`Unknown tool: ${name}`)
  }
}

// ── Version check ───────────────────────────────

async function checkForUpdate(): Promise<{ latest: string; outdated: boolean } | null> {
  try {
    const res = await fetch('https://registry.npmjs.org/@opengate/mcp/latest', {
      headers: { 'Accept': 'application/json' },
      signal: AbortSignal.timeout(5000),
    })
    if (!res.ok) return null
    const data = await res.json() as { version?: string }
    const latest = data.version
    if (!latest) return null
    return { latest, outdated: latest !== PKG_VERSION }
  } catch {
    return null
  }
}

let updateCheckInterval: ReturnType<typeof setInterval> | null = null

function startPeriodicUpdateCheck() {
  updateCheckInterval = setInterval(async () => {
    const result = await checkForUpdate()
    if (result?.outdated) {
      process.stderr.write(
        `[opengate-mcp] Update available: ${PKG_VERSION} → ${result.latest}. Restart your MCP client to update.\n`
      )
      if (updateCheckInterval) {
        clearInterval(updateCheckInterval)
        updateCheckInterval = null
      }
    }
  }, 7_200_000)
}

// ── MCP protocol handler ────────────────────────

async function handleInitialize() {
  // Send heartbeat on connect so the dashboard detects the agent immediately
  post('/api/agents/heartbeat').catch((err) => {
    process.stderr.write(`[opengate-mcp] heartbeat failed: ${err instanceof Error ? err.message : err}\n`)
  })

  // Fire-and-forget version check on startup
  checkForUpdate().then((result) => {
    if (result?.outdated) {
      process.stderr.write(
        `[opengate-mcp] Update available: ${PKG_VERSION} → ${result.latest}. Restart your MCP client to update.\n`
      )
    }
  }).catch(() => {})

  // Re-check every 2 hours (clears itself once an update is found)
  startPeriodicUpdateCheck()

  return {
    protocolVersion: '2024-11-05',
    capabilities: { tools: {} },
    serverInfo: { name: 'opengate', version: PKG_VERSION },
  }
}

function handleToolsList() {
  return { tools: TOOLS }
}

async function handleToolsCall(params: { name?: string; arguments?: Record<string, unknown> }) {
  const toolName = params.name ?? ''
  const args = { ...(params.arguments ?? {}) }

  // Auto-inject project_id when bridge is project-scoped
  if (projectId && PROJECT_SCOPED_TOOLS.has(toolName) && !args.project_id) {
    args.project_id = projectId
  }

  try {
    const result = await dispatch(toolName, args)
    return {
      content: [{
        type: 'text',
        text: JSON.stringify(result, null, 2),
      }],
    }
  } catch (err) {
    return {
      content: [{
        type: 'text',
        text: err instanceof Error ? err.message : String(err),
      }],
      isError: true,
    }
  }
}

// ── Stdio transport ─────────────────────────────

const rl = createInterface({ input: process.stdin })

rl.on('line', async (line) => {
  if (!line.trim()) return

  let request: { id?: unknown; method?: string; params?: Record<string, unknown> }
  try {
    request = JSON.parse(line)
  } catch {
    send({ jsonrpc: '2.0', id: null, error: { code: -32700, message: 'Parse error' } })
    return
  }

  // Notifications (no id) — just acknowledge
  if (request.id === undefined || request.id === null) {
    // Handle notifications like 'notifications/initialized' silently
    return
  }

  let result: unknown
  try {
    switch (request.method) {
      case 'initialize':
        result = await handleInitialize()
        break
      case 'tools/list':
        result = handleToolsList()
        break
      case 'tools/call':
        result = await handleToolsCall(request.params as { name?: string; arguments?: Record<string, unknown> })
        break
      case 'ping':
        result = {}
        break
      default:
        send({ jsonrpc: '2.0', id: request.id, error: { code: -32601, message: `Method not found: ${request.method}` } })
        return
    }
  } catch (err) {
    send({ jsonrpc: '2.0', id: request.id, error: { code: -32603, message: err instanceof Error ? err.message : String(err) } })
    return
  }

  send({ jsonrpc: '2.0', id: request.id, result })
})

function send(msg: unknown) {
  process.stdout.write(JSON.stringify(msg) + '\n')
}

// ── Helpers ─────────────────────────────────────

function tool(
  name: string,
  description: string,
  properties: Record<string, unknown>,
  required?: string[],
) {
  return {
    name,
    description,
    inputSchema: {
      type: 'object',
      properties,
      ...(required?.length ? { required } : {}),
    },
  }
}

function parseArg(flag: string): string | undefined {
  const args = process.argv.slice(2)
  for (let i = 0; i < args.length; i++) {
    if (args[i] === flag && args[i + 1]) return args[i + 1]
    if (args[i]?.startsWith(`${flag}=`)) return args[i].slice(flag.length + 1)
  }
  return undefined
}
