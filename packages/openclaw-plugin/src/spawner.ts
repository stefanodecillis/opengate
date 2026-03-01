import type { OpenClawConfig } from "openclaw/plugin-sdk";
import type { OpenGatePluginConfig } from "./config.js";

type SpawnResult = { ok: true; sessionKey: string } | { ok: false; error: string };

/**
 * Spawns an isolated OpenClaw session for a task by POSTing to the local /hooks/agent endpoint.
 * Requires hooks to be enabled in the OpenClaw config with a valid token.
 */
export async function spawnTaskSession(
  taskId: string,
  message: string,
  pluginCfg: OpenGatePluginConfig,
  openclawCfg: OpenClawConfig,
): Promise<SpawnResult> {
  // Resolve gateway URL + hooks token from OpenClaw config
  const port = (openclawCfg as any)?.gateway?.port ?? 18789;
  const hooksToken = (openclawCfg as any)?.hooks?.token;

  if (!hooksToken) {
    return {
      ok: false,
      error:
        "[opengate] hooks.token not configured in OpenClaw config. " +
        "Add hooks.enabled=true and hooks.token=<secret> to enable task spawning.",
    };
  }

  const sessionKey = `opengate-task:${taskId}`;
  const agentId = pluginCfg.agentId ?? "main";

  const payload: Record<string, unknown> = {
    message,
    agentId,
    sessionKey,
    wakeMode: "now",
    deliver: false,
    name: "OpenGate",
  };

  if (pluginCfg.model) {
    payload.model = pluginCfg.model;
  }

  try {
    const resp = await fetch(`http://127.0.0.1:${port}/hooks/agent`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${hooksToken}`,
      },
      body: JSON.stringify(payload),
    });

    if (resp.status === 202 || resp.status === 200) {
      return { ok: true, sessionKey };
    }

    const body = await resp.text().catch(() => "(no body)");
    return {
      ok: false,
      error: `[opengate] hooks/agent returned HTTP ${resp.status}: ${body}`,
    };
  } catch (e) {
    return {
      ok: false,
      error: `[opengate] failed to reach hooks/agent: ${e instanceof Error ? e.message : String(e)}`,
    };
  }
}
