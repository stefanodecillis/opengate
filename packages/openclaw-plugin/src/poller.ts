import type { OpenClawConfig } from "openclaw/plugin-sdk";
import type { PluginLogger } from "openclaw/plugin-sdk";
import type { OpenGatePluginConfig } from "./config.js";
import { buildBootstrapPrompt } from "./bootstrap.js";
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

async function fetchInbox(url: string, apiKey: string): Promise<OpenGateTask[]> {
  const resp = await fetch(`${url}/api/agents/me/inbox`, {
    headers: { Authorization: `Bearer ${apiKey}` },
    signal: AbortSignal.timeout(10_000),
  });

  if (!resp.ok) {
    throw new Error(`OpenGate inbox returned HTTP ${resp.status}`);
  }

  const body = (await resp.json()) as InboxResponse;
  return body.todo_tasks ?? [];
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

    const maxConcurrent = this.pluginCfg.maxConcurrent ?? 3;
    const active = this.state.activeCount();

    if (active >= maxConcurrent) {
      this.logger.info(
        `[opengate] At capacity (${active}/${maxConcurrent} active) — skipping poll`,
      );
      return;
    }

    let tasks: OpenGateTask[];
    try {
      tasks = await fetchInbox(this.pluginCfg.url, this.pluginCfg.apiKey);
    } catch (e) {
      this.logger.warn(
        `[opengate] Failed to fetch inbox: ${e instanceof Error ? e.message : String(e)}`,
      );
      return;
    }

    if (tasks.length === 0) return;

    this.logger.info(`[opengate] Found ${tasks.length} todo task(s)`);

    for (const task of tasks) {
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
