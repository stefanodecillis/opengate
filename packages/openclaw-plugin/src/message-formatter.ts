/**
 * Formats OpenGate events into human-readable messages with MCP tool instructions.
 */

export interface OpenGateEvent {
  type: string;
  task_id?: string;
  task_title?: string;
  project_id?: string;
  from_agent?: string;
  reason?: string;
  summary?: string;
  content?: string;
  priority?: string;
  tags?: string[];
  [key: string]: unknown;
}

export interface Notification {
  id: string;
  event_type: string;
  payload: OpenGateEvent;
  read: boolean;
  created_at: string;
}

/**
 * Format a single OpenGate event into a readable message for the agent.
 */
export function formatEvent(event: OpenGateEvent): string {
  const taskRef = event.task_id
    ? ` (task: ${event.task_title ?? event.task_id})`
    : "";

  switch (event.type) {
    case "task.assigned":
      return [
        `New task assigned to you${taskRef}`,
        event.priority ? `Priority: ${event.priority}` : null,
        event.tags?.length ? `Tags: ${event.tags.join(", ")}` : null,
        event.repo_url ? `Repo: ${event.repo_url}` : null,
        "",
        "Next steps:",
        "1. Use `get_task` to read the full task details",
        event.project_id
          ? "2. Use `get_workspace_info` to set up project workspace"
          : null,
        "3. Use `search_knowledge` to check for relevant project knowledge",
        "4. Use `claim_task` to start working on it",
        "5. Use `post_comment` to share your planned approach",
      ]
        .filter((line) => line !== null)
        .join("\n");

    case "task.comment":
      return [
        `New comment on task${taskRef}`,
        event.from_agent ? `From: ${event.from_agent}` : null,
        event.content ? `> ${event.content}` : null,
        "",
        "Use `get_task` to see the full task context.",
      ]
        .filter((line) => line !== null)
        .join("\n");

    case "task.dependency_ready":
      return [
        `Dependency resolved for task${taskRef}`,
        "A blocking dependency has been completed. This task may now be ready to claim.",
        "",
        "Next steps:",
        "1. Use `get_task` to review the task",
        "2. Use `list_dependencies` to verify all dependencies are met",
        "3. Use `claim_task` to start working if ready",
      ].join("\n");

    case "task.review_requested":
      return [
        `Review requested for task${taskRef}`,
        event.from_agent ? `From: ${event.from_agent}` : null,
        event.summary ? `Summary: ${event.summary}` : null,
        "",
        "Next steps:",
        "1. Use `get_task` to review the task and its output",
        "2. Approve with `approve_task` or request changes with `request_changes`",
      ]
        .filter((line) => line !== null)
        .join("\n");

    case "task.handoff":
      return [
        `Task handed off to you${taskRef}`,
        event.from_agent ? `From: ${event.from_agent}` : null,
        event.summary ? `Context: ${event.summary}` : null,
        "",
        "Next steps:",
        "1. Use `get_task` to read the full task and handoff context",
        "2. Use `claim_task` to accept the handoff",
        "3. Use `post_comment` to acknowledge and share your plan",
      ]
        .filter((line) => line !== null)
        .join("\n");

    case "task.unblocked":
      return [
        `Task unblocked${taskRef}`,
        event.reason ? `Reason: ${event.reason}` : null,
        "",
        "The task has been unblocked and moved back to your queue.",
        "Use `get_task` to review and continue working on it.",
      ]
        .filter((line) => line !== null)
        .join("\n");

    case "task.changes_requested":
      return [
        `Changes requested on task${taskRef}`,
        event.from_agent ? `From: ${event.from_agent}` : null,
        event.content ? `Feedback: ${event.content}` : null,
        "",
        "Next steps:",
        "1. Use `get_task` to read the review feedback",
        "2. Address the requested changes",
        "3. Use `complete_task` to resubmit when ready",
      ]
        .filter((line) => line !== null)
        .join("\n");

    default:
      return [
        `OpenGate event: ${event.type}${taskRef}`,
        event.summary ?? event.content ?? "",
        "",
        "Use `check_inbox` to see your current task queue.",
      ]
        .filter((line) => line !== "")
        .join("\n");
  }
}

/**
 * Format a list of notifications into a summary message.
 */
export function formatNotificationSummary(
  notifications: Notification[]
): string {
  if (notifications.length === 0) {
    return "";
  }

  const lines: string[] = [
    `You have ${notifications.length} unread notification${notifications.length === 1 ? "" : "s"} from OpenGate:`,
    "",
  ];

  for (const notification of notifications) {
    const formatted = formatEvent(notification.payload);
    lines.push(`--- ${notification.event_type} ---`);
    lines.push(formatted);
    lines.push("");
  }

  lines.push(
    "Use `check_inbox` for a full overview of your task queue."
  );

  return lines.join("\n");
}
