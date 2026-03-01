import type { OpenClawPluginApi } from "openclaw/plugin-sdk";
import { resolveConfig } from "./config.js";
import { OpenGatePoller } from "./poller.js";

export default function register(api: OpenClawPluginApi): void {
  let poller: OpenGatePoller | null = null;

  // Validate config early — log clear errors if misconfigured
  let pluginCfg: ReturnType<typeof resolveConfig>;
  try {
    pluginCfg = resolveConfig(api.pluginConfig ?? {});
  } catch (e) {
    api.logger.error(e instanceof Error ? e.message : String(e));
    return;
  }

  // Check hooks are enabled — required to spawn sessions
  const hooksToken = (api.config as any)?.hooks?.token;
  if (!hooksToken) {
    api.logger.error(
      "[opengate] hooks.token is not configured. " +
      "Add the following to your OpenClaw config to enable task spawning:\n" +
      '  "hooks": { "enabled": true, "token": "<your-secret>", ' +
      '"allowRequestSessionKey": true, "allowedSessionKeyPrefixes": ["opengate-task:"] }',
    );
    return;
  }

  api.registerService({
    id: "opengate-poller",

    start(ctx) {
      poller = new OpenGatePoller(pluginCfg, api.config, ctx.logger, ctx.stateDir);
      poller.start();
    },

    stop(ctx) {
      poller?.stop();
      poller = null;
    },
  });
}
