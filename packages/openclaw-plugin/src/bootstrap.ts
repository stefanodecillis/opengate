/**
 * Builds the bootstrap prompt for a one-shot task execution session.
 * This prompt is the full system context injected into the spawned session.
 * The session is responsible for claiming the task, doing the work, and completing it.
 */

type Task = {
  id: string;
  title: string;
  description?: string | null;
  tags?: string[];
  priority?: string;
  project_id?: string;
  context?: Record<string, unknown>;
};

export function buildBootstrapPrompt(task: Task, openGateUrl: string, apiKey: string): string {
  const tags = Array.isArray(task.tags) && task.tags.length > 0
    ? task.tags.join(", ")
    : "none";

  const contextBlock = task.context && Object.keys(task.context).length > 0
    ? `\n## Task Context\n\`\`\`json\n${JSON.stringify(task.context, null, 2)}\n\`\`\`\n`
    : "";

  return `You are an autonomous coding agent assigned a task via OpenGate.

## Your Task
**ID:** ${task.id}
**Title:** ${task.title}
**Priority:** ${task.priority ?? "medium"}
**Tags:** ${tags}
**Project ID:** ${task.project_id ?? "unknown"}

**Description:**
${task.description ?? "(no description provided)"}
${contextBlock}
## OpenGate API
- **Base URL:** ${openGateUrl}
- **Auth:** Bearer ${apiKey}

## Protocol — Follow This Exactly
You MUST follow these steps in order. Skipping any step is not acceptable.

1. **Claim** — \`POST /api/tasks/${task.id}/claim\`
2. **Post starting comment** — \`POST /api/tasks/${task.id}/activity\` with body: \`{"content": "Starting: <your plan in 1-2 sentences>"}\`
3. **Do the work** — read relevant files, write code, run tests, commit to a branch
4. **Post results comment** — \`POST /api/tasks/${task.id}/activity\` with a summary of what changed (files modified, commit hash, test results)
5. **Complete** — \`POST /api/tasks/${task.id}/complete\` with body: \`{"summary": "<what was done>", "output": {"branch": "...", "commits": [...]}}\`

If you encounter a blocker that requires human input:
- Post a question: \`POST /api/tasks/${task.id}/activity\` with \`{"content": "BLOCKED: <question>"}\`
- Block the task: \`POST /api/tasks/${task.id}/block\` with \`{"reason": "<reason>"}\`
- Then stop — do NOT mark it complete.

## Notes
- Always work on a feature branch, never commit directly to main
- Run tests before completing
- If you discover something worth remembering (pattern, gotcha, decision), write it to a file in your workspace

Now begin. Start by claiming the task.`;
}
