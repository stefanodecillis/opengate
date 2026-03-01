/**
 * HTTP polling client for OpenGate notifications.
 * Polls GET /api/agents/me/notifications?unread=true at a configurable interval.
 */

import type { Notification } from "./message-formatter.js";

export interface PollerConfig {
  url: string;
  apiKey: string;
  pollIntervalMs: number;
  projectId?: string;
}

export type NotificationHandler = (notifications: Notification[]) => void;

export class Poller {
  private config: PollerConfig;
  private handler: NotificationHandler;
  private timer: ReturnType<typeof setInterval> | null = null;
  private running = false;

  constructor(config: PollerConfig, handler: NotificationHandler) {
    this.config = config;
    this.handler = handler;
  }

  start(): void {
    if (this.running) return;
    this.running = true;

    // Poll immediately on start
    void this.poll();

    // Then poll at the configured interval
    this.timer = setInterval(() => {
      void this.poll();
    }, this.config.pollIntervalMs);
  }

  stop(): void {
    this.running = false;
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }

  private async poll(): Promise<void> {
    if (!this.running) return;

    try {
      const baseUrl = this.config.url.replace(/\/$/, "");
      const params = new URLSearchParams({ unread: "true" });
      if (this.config.projectId) {
        params.set("project_id", this.config.projectId);
      }

      const response = await fetch(
        `${baseUrl}/api/agents/me/notifications?${params.toString()}`,
        {
          headers: {
            Authorization: `Bearer ${this.config.apiKey}`,
            "Content-Type": "application/json",
          },
        }
      );

      if (!response.ok) {
        console.error(
          `[opengate-bridge] Poll failed: ${response.status} ${response.statusText}`
        );
        return;
      }

      const notifications = (await response.json()) as Notification[];
      if (notifications.length > 0) {
        this.handler(notifications);

        // Acknowledge notifications so they aren't returned again
        await this.acknowledgeNotifications(
          baseUrl,
          notifications.map((n) => n.id)
        );
      }
    } catch (err) {
      console.error(`[opengate-bridge] Poll error:`, err);
    }
  }

  private async acknowledgeNotifications(
    baseUrl: string,
    ids: string[]
  ): Promise<void> {
    try {
      for (const id of ids) {
        await fetch(`${baseUrl}/api/agents/me/notifications/${id}/ack`, {
          method: "POST",
          headers: {
            Authorization: `Bearer ${this.config.apiKey}`,
            "Content-Type": "application/json",
          },
        });
      }
    } catch (err) {
      console.error(`[opengate-bridge] Failed to acknowledge notifications:`, err);
    }
  }
}
