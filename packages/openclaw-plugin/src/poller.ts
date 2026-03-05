import type { OpenClawConfig } from "openclaw/plugin-sdk";
import type { PluginLogger } from "openclaw/plugin-sdk";
import type { OpenGatePluginConfig } from "./config.js";
import { buildBootstrapPrompt, buildMentionPrompt } from "./bootstrap.js";
import { spawnTaskSession } from "./spawner.js";
import { TaskState } from "./state.js";

type OpenGateTask = {
  id: string;
  title: string;
  description?: string | null;
  tags?: string[];
  priority?: string;
  project_id?: string;
  status?: string;
  context?: Record<string, unknown>;
};

type InboxResponse = {
  todo_tasks?: OpenGateTask[];
  in_progress_tasks?: OpenGateTask[];
};

export type ProjectInfo = {
  id: string;
  name: string;
  repo_url?: string | null;
  default_branch?: string | null;
};

async function fetchProject(url: string, apiKey: string, projectId: string): Promise<ProjectInfo | null> {
  try {
    const resp = await fetch(`${url}/api/projects/${projectId}`, {
      headers: { Authorization: `Bearer ${apiKey}` },
      signal: AbortSignal.timeout(10_000),
    });
    if (!resp.ok) return null;
    const body = (await resp.json()) as { project?: ProjectInfo } & ProjectInfo;
    // The endpoint returns ProjectWithStats which wraps project, but handle both shapes
    return body.project ?? body;
  } catch {
    return null;
  }
}

type InboxResult = {
  todoTasks: OpenGateTask[];
  inProgressTasks: OpenGateTask[];
};

async function fetchInbox(url: string, apiKey: string): Promise<InboxResult> {
  const resp = await fetch(`${url}/api/agents/me/inbox`, {
    headers: { Authorization: `Bearer ${apiKey}` },
    signal: AbortSignal.timeout(10_000),
  });

  if (!resp.ok) {
    throw new Error(`OpenGate inbox returned HTTP ${resp.status}`);
  }

  const body = (await resp.json()) as InboxResponse;
  return {
    todoTasks: body.todo_tasks ?? [],
    inProgressTasks: body.in_progress_tasks ?? [],
  };
}

type OpenGateNotification = {
  id: number;
  event_type: string;
  title: string;
  body: string | null;
  task_id: string | null;
};

async function fetchNotifications(url: string, apiKey: string): Promise<OpenGateNotification[]> {
  const resp = await fetch(`${url}/api/agents/me/notifications?unread=true`, {
    headers: { Authorization: `Bearer ${apiKey}` },
    signal: AbortSignal.timeout(10_000),
  });
  if (!resp.ok) return [];
  return (await resp.json()) as OpenGateNotification[];
}

async function ackNotification(url: string, apiKey: string, notifId: number): Promise<void> {
  await fetch(`${url}/api/agents/me/notifications/${notifId}/ack`, {
    method: "POST",
    headers: { Authorization: `Bearer ${apiKey}` },
    signal: AbortSignal.timeout(10_000),
  }).catch(() => {});
}

async function fetchTaskById(
  url: string,
  apiKey: string,
  taskId: string,
): Promise<OpenGateTask | null> {
  try {
    const resp = await fetch(`${url}/api/tasks/${taskId}`, {
      headers: { Authorization: `Bearer ${apiKey}` },
      signal: AbortSignal.timeout(10_000),
    });
    if (!resp.ok) return null;
    return (await resp.json()) as OpenGateTask;
  } catch {
    return null;
  }
}

export class OpenGatePoller {
  private timer: ReturnType<typeof setInterval> | null = null;
  private state: TaskState;
  private running = false;
  private projectCache = new Map<string, ProjectInfo>();

  constructor(
    private pluginCfg: OpenGatePluginConfig,
    private openclawCfg: OpenClawConfig,
    private logger: PluginLogger,
    stateDir: string,
  ) {
    this.state = new TaskState(stateDir);
  }

  start(): void {
    if (this.running) return;
    this.running = true;

    const intervalMs = this.pluginCfg.pollIntervalMs ?? 30_000;
    this.logger.info(`[opengate] Starting poller — interval: ${intervalMs}ms`);

    // Run immediately, then on interval
    void this.poll();
    this.timer = setInterval(() => void this.poll(), intervalMs);
  }

  stop(): void {
    this.running = false;
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
    this.logger.info("[opengate] Poller stopped");
  }

  private async poll(): Promise<void> {
    if (!this.running) return;

    let inbox: InboxResult;
    try {
      inbox = await fetchInbox(this.pluginCfg.url, this.pluginCfg.apiKey);
    } catch (e) {
      this.logger.warn(
        `[opengate] Failed to fetch inbox: ${e instanceof Error ? e.message : String(e)}`,
      );
      return;
    }

    // Reconcile: remove spawned entries for tasks no longer in the inbox.
    // If a task completed, got cancelled, or was otherwise removed from our
    // inbox, it won't appear in todo or in_progress — free the slot.
    const inboxTaskIds = new Set([
      ...inbox.todoTasks.map((t) => t.id),
      ...inbox.inProgressTasks.map((t) => t.id),
    ]);
    for (const spawnedId of this.state.spawnedIds()) {
      if (!inboxTaskIds.has(spawnedId)) {
        this.logger.info(
          `[opengate] Task ${spawnedId} no longer in inbox (completed/cancelled) — freeing slot`,
        );
        this.state.remove(spawnedId);
      }
    }

    // Release orphaned in_progress tasks (not tracked locally) and clean state
    for (const task of inbox.inProgressTasks) {
      if (!this.state.isSpawned(task.id)) {
        this.logger.warn(
          `[opengate] Orphaned in_progress task "${task.title}" (${task.id}) — releasing back to todo`,
        );
        await this.releaseTask(task.id);
      }
    }

    // Handle unread notifications (mentions, etc.) — these spawn separate sessions
    await this.handleNotifications();

    const maxConcurrent = this.pluginCfg.maxConcurrent ?? 3;
    const active = this.state.activeCount();

    if (active >= maxConcurrent) {
      this.logger.info(
        `[opengate] At capacity (${active}/${maxConcurrent} active) — skipping poll`,
      );
      return;
    }

    if (inbox.todoTasks.length === 0) return;

    this.logger.info(`[opengate] Found ${inbox.todoTasks.length} todo task(s)`);

    for (const task of inbox.todoTasks) {
      if (!this.running) break;

      const currentActive = this.state.activeCount();
      if (currentActive >= maxConcurrent) {
        this.logger.info(
          `[opengate] Reached capacity (${currentActive}/${maxConcurrent}) — deferring remaining tasks`,
        );
        break;
      }

      if (this.state.isSpawned(task.id)) {
        this.logger.info(`[opengate] Task ${task.id} already spawned — skipping`);
        continue;
      }

      await this.spawnTask(task);
    }
  }

  private async resolveProject(projectId: string): Promise<ProjectInfo | null> {
    const cached = this.projectCache.get(projectId);
    if (cached) return cached;

    const project = await fetchProject(this.pluginCfg.url, this.pluginCfg.apiKey, projectId);
    if (project) this.projectCache.set(projectId, project);
    return project;
  }

  private async releaseTask(taskId: string): Promise<void> {
    try {
      const resp = await fetch(
        `${this.pluginCfg.url}/api/tasks/${taskId}/release`,
        {
          method: "POST",
          headers: { Authorization: `Bearer ${this.pluginCfg.apiKey}` },
          signal: AbortSignal.timeout(10_000),
        },
      );
      if (!resp.ok) {
        this.logger.warn(`[opengate] Failed to release task ${taskId}: HTTP ${resp.status}`);
      }
    } catch (e) {
      this.logger.warn(
        `[opengate] Failed to release task ${taskId}: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
  }

  private async handleNotifications(): Promise<void> {
    let notifications: OpenGateNotification[];
    try {
      notifications = await fetchNotifications(this.pluginCfg.url, this.pluginCfg.apiKey);
    } catch {
      return;
    }

    const mentionNotifs = notifications.filter(
      (n) => n.event_type === "task.comment_mention" && n.task_id,
    );

    for (const notif of mentionNotifs) {
      if (!this.running) break;

      const maxConcurrent = this.pluginCfg.maxConcurrent ?? 3;
      if (this.state.activeCount() >= maxConcurrent) {
        this.logger.info("[opengate] At capacity — deferring mention notifications");
        break;
      }

      // Use a mention-specific key to avoid collision with regular task sessions
      const mentionKey = `mention:${notif.id}`;
      if (this.state.isSpawned(mentionKey)) continue;

      await this.spawnMentionSession(notif);
      await ackNotification(this.pluginCfg.url, this.pluginCfg.apiKey, notif.id);
    }
  }

  private async spawnMentionSession(notif: OpenGateNotification): Promise<void> {
    const taskId = notif.task_id!;
    this.logger.info(
      `[opengate] Handling @-mention notification ${notif.id} on task ${taskId}`,
    );

    // Parse author and body from notification body (format: "author: comment")
    const bodyText = notif.body ?? "";
    const colonIdx = bodyText.indexOf(": ");
    const author = colonIdx > 0 ? bodyText.slice(0, colonIdx) : "Someone";
    const commentBody = colonIdx > 0 ? bodyText.slice(colonIdx + 2) : bodyText;

    // Resolve project from the task
    const task = await fetchTaskById(this.pluginCfg.url, this.pluginCfg.apiKey, taskId);
    let project: ProjectInfo | null = null;
    if (task?.project_id) {
      project = await this.resolveProject(task.project_id);
    }

    const prompt = buildMentionPrompt(
      taskId,
      commentBody,
      author,
      this.pluginCfg.url,
      this.pluginCfg.apiKey,
      project,
      this.pluginCfg.projectsDir,
    );

    const result = await spawnTaskSession(
      `mention-${notif.id}`,
      prompt,
      this.pluginCfg,
      this.openclawCfg,
    );

    if (!result.ok) {
      this.logger.error(result.error);
      return;
    }

    const mentionKey = `mention:${notif.id}`;
    this.state.markSpawned(mentionKey, result.sessionKey);
    this.logger.info(
      `[opengate] Mention session spawned for notification ${notif.id} → ${result.sessionKey}`,
    );
  }

  private async spawnTask(task: OpenGateTask): Promise<void> {
    this.logger.info(`[opengate] Spawning session for task: "${task.title}" (${task.id})`);

    // Resolve project info to get repo_url → local workspace path
    let project: ProjectInfo | null = null;
    if (task.project_id) {
      project = await this.resolveProject(task.project_id);
    }

    const prompt = buildBootstrapPrompt(
      task,
      this.pluginCfg.url,
      this.pluginCfg.apiKey,
      project,
      this.pluginCfg.projectsDir,
    );

    const result = await spawnTaskSession(
      task.id,
      prompt,
      this.pluginCfg,
      this.openclawCfg,
    );

    if (!result.ok) {
      this.logger.error(result.error);
      return;
    }

    this.state.markSpawned(task.id, result.sessionKey);
    this.logger.info(
      `[opengate] Session spawned for task ${task.id} → session key: ${result.sessionKey}`,
    );
  }
}
