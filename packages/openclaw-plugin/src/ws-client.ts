/**
 * WebSocket client for OpenGate real-time notifications.
 * Connects to /api/ws, authenticates, and subscribes to agent events.
 * Features auto-reconnect with exponential backoff (1s -> 2s -> 4s -> ... -> max 60s).
 */

import WebSocket from "ws";
import type { OpenGateEvent } from "./message-formatter.js";

export interface WsClientConfig {
  url: string;
  apiKey: string;
  projectId?: string;
}

export type EventHandler = (event: OpenGateEvent) => void;

const INITIAL_BACKOFF_MS = 1000;
const MAX_BACKOFF_MS = 60000;
const BACKOFF_MULTIPLIER = 2;

export class WsClient {
  private config: WsClientConfig;
  private handler: EventHandler;
  private ws: WebSocket | null = null;
  private running = false;
  private backoffMs = INITIAL_BACKOFF_MS;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private pingTimer: ReturnType<typeof setInterval> | null = null;

  constructor(config: WsClientConfig, handler: EventHandler) {
    this.config = config;
    this.handler = handler;
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    this.connect();
  }

  stop(): void {
    this.running = false;

    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }

    if (this.pingTimer) {
      clearInterval(this.pingTimer);
      this.pingTimer = null;
    }

    if (this.ws) {
      this.ws.close(1000, "Plugin stopped");
      this.ws = null;
    }
  }

  private connect(): void {
    if (!this.running) return;

    const baseUrl = this.config.url.replace(/\/$/, "").replace(/^http/, "ws");
    const wsUrl = `${baseUrl}/api/ws`;

    try {
      this.ws = new WebSocket(wsUrl, {
        headers: {
          Authorization: `Bearer ${this.config.apiKey}`,
        },
      });

      this.ws.on("open", () => {
        console.log("[opengate-bridge] WebSocket connected");
        this.backoffMs = INITIAL_BACKOFF_MS;

        // Send subscribe message
        const subscribe = JSON.stringify({
          type: "subscribe",
          channels: ["agent.notifications"],
          project_id: this.config.projectId,
        });
        this.ws?.send(subscribe);

        // Start ping to keep connection alive
        this.pingTimer = setInterval(() => {
          if (this.ws?.readyState === WebSocket.OPEN) {
            this.ws.ping();
          }
        }, 30000);
      });

      this.ws.on("message", (data: WebSocket.Data) => {
        try {
          const event = JSON.parse(data.toString()) as OpenGateEvent;
          if (event.type && event.type !== "pong") {
            this.handler(event);
          }
        } catch {
          console.error(
            "[opengate-bridge] Failed to parse WebSocket message"
          );
        }
      });

      this.ws.on("close", (code: number, reason: Buffer) => {
        console.log(
          `[opengate-bridge] WebSocket closed: ${code} ${reason.toString()}`
        );
        this.cleanup();
        this.scheduleReconnect();
      });

      this.ws.on("error", (err: Error) => {
        console.error(`[opengate-bridge] WebSocket error:`, err.message);
        this.cleanup();
        this.scheduleReconnect();
      });
    } catch (err) {
      console.error(`[opengate-bridge] WebSocket connection failed:`, err);
      this.scheduleReconnect();
    }
  }

  private cleanup(): void {
    if (this.pingTimer) {
      clearInterval(this.pingTimer);
      this.pingTimer = null;
    }
    this.ws = null;
  }

  private scheduleReconnect(): void {
    if (!this.running) return;
    if (this.reconnectTimer) return;

    console.log(
      `[opengate-bridge] Reconnecting in ${this.backoffMs / 1000}s...`
    );

    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.backoffMs = Math.min(
        this.backoffMs * BACKOFF_MULTIPLIER,
        MAX_BACKOFF_MS
      );
      this.connect();
    }, this.backoffMs);
  }
}
