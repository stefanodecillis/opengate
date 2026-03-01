/**
 * @opengate/openclaw — OpenClaw plugin for OpenGate agent notifications.
 *
 * Provides real-time push notifications from OpenGate to agents via
 * HTTP polling (default) or WebSocket.
 *
 * Registers a service "opengate-bridge" that:
 * - Connects to an OpenGate instance
 * - Listens for agent events (task assignments, comments, unblocks, etc.)
 * - Injects formatted messages into the agent's session via sessions.send()
 */

const PKG_VERSION = '0.1.8'

import { Poller } from "./poller.js";
import { WsClient } from "./ws-client.js";
import {
  formatEvent,
  formatNotificationSummary,
  type Notification,
  type OpenGateEvent,
} from "./message-formatter.js";

export interface PluginConfig {
  url: string;
  apiKey: string;
  mode?: "polling" | "websocket";
  pollIntervalMs?: number;
  projectId?: string;
}

/**
 * OpenClaw Plugin API surface (subset relevant to this plugin).
 * These types represent what OpenClaw provides to plugins.
 */
export interface OpenClawPluginApi {
  registerService(service: {
    id: string;
    start: () => void | Promise<void>;
    stop: () => void | Promise<void>;
  }): void;
  gateway: {
    rpc(
      method: string,
      params: Record<string, unknown>
    ): Promise<unknown>;
  };
}

async function checkForUpdate(): Promise<void> {
  try {
    const res = await fetch('https://registry.npmjs.org/@opengate/openclaw/latest', {
      headers: { 'Accept': 'application/json' },
      signal: AbortSignal.timeout(5000),
    })
    if (!res.ok) return
    const data = await res.json() as { version?: string }
    const latest = data.version
    if (latest && latest !== PKG_VERSION) {
      console.log(`[opengate-bridge] Update available: ${PKG_VERSION} → ${latest}. Run: openclaw plugins update`)
    }
  } catch {
    // Network error — silently skip
  }
}

const DEFAULT_POLL_INTERVAL_MS = 600_000; // 10 minutes
const SESSION_KEY = "main";

/**
 * Plugin entry point. Called by OpenClaw when the plugin is loaded.
 */
export default function register(
  api: OpenClawPluginApi,
  config: PluginConfig
): void {
  const mode = config.mode ?? "polling";
  const pollIntervalMs = config.pollIntervalMs ?? DEFAULT_POLL_INTERVAL_MS;

  let poller: Poller | null = null;
  let wsClient: WsClient | null = null;

  function sendMessage(message: string): void {
    if (!message) return;
    void api.gateway.rpc("sessions.send", {
      sessionKey: SESSION_KEY,
      message,
    });
  }

  function handleNotifications(notifications: Notification[]): void {
    const summary = formatNotificationSummary(notifications);
    sendMessage(summary);
  }

  function handleWsEvent(event: OpenGateEvent): void {
    const message = formatEvent(event);
    sendMessage(message);
  }

  api.registerService({
    id: "opengate-bridge",

    start() {
      console.log(
        `[opengate-bridge] Starting in ${mode} mode (url: ${config.url})`
      );

      // Fire-and-forget version check
      checkForUpdate().catch(() => {});

      if (mode === "websocket") {
        wsClient = new WsClient(
          {
            url: config.url,
            apiKey: config.apiKey,
            projectId: config.projectId,
          },
          handleWsEvent
        );
        wsClient.start();
      } else {
        poller = new Poller(
          {
            url: config.url,
            apiKey: config.apiKey,
            pollIntervalMs,
            projectId: config.projectId,
          },
          handleNotifications
        );
        poller.start();
      }
    },

    stop() {
      console.log("[opengate-bridge] Stopping");
      poller?.stop();
      wsClient?.stop();
      poller = null;
      wsClient = null;
    },
  });
}

// Re-export types and utilities for consumers
export { formatEvent, formatNotificationSummary } from "./message-formatter.js";
export type { OpenGateEvent, Notification } from "./message-formatter.js";
export type { PollerConfig } from "./poller.js";
export type { WsClientConfig } from "./ws-client.js";
