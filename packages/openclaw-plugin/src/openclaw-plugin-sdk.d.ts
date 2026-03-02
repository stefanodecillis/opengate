declare module "openclaw/plugin-sdk" {
  export interface PluginLogger {
    info(message: string): void;
    warn(message: string): void;
    error(message: string): void;
    debug(message: string): void;
  }

  export interface ServiceContext {
    logger: PluginLogger;
    stateDir: string;
  }

  export interface OpenClawPluginConfigSchema {
    safeParse(value: unknown): { success: true; data: unknown } | { success: false; error: string };
    jsonSchema: Record<string, unknown>;
  }

  export interface OpenClawPluginApi {
    /** Unique plugin ID */
    id: string;
    /** Human-readable plugin name */
    name: string;
    /** Runtime information from the host */
    runtime: Record<string, unknown>;
    /** Plugin-specific config from openclaw.json */
    pluginConfig: Record<string, unknown> | undefined;
    /** Global OpenClaw config */
    config: OpenClawConfig;
    /** Plugin-scoped logger */
    logger: PluginLogger;

    registerService(service: {
      id: string;
      start: (ctx: ServiceContext) => void | Promise<void>;
      stop: (ctx: ServiceContext) => void | Promise<void>;
    }): void;

    registerProvider(provider: {
      id: string;
      [key: string]: unknown;
    }): void;

    registerChannel(channel: {
      id: string;
      [key: string]: unknown;
    }): void;
  }

  export interface OpenClawConfig {
    [key: string]: unknown;
  }

  /** Returns a permissive empty config schema (no runtime validation). */
  export function emptyPluginConfigSchema(): OpenClawPluginConfigSchema;
}
