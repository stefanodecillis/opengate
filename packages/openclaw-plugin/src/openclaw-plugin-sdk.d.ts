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

  export interface OpenClawPluginApi {
    pluginConfig: Record<string, unknown> | undefined;
    config: OpenClawConfig;
    logger: PluginLogger;
    registerService(service: {
      id: string;
      start: (ctx: ServiceContext) => void | Promise<void>;
      stop: (ctx: ServiceContext) => void | Promise<void>;
    }): void;
  }

  export interface OpenClawConfig {
    [key: string]: unknown;
  }
}
