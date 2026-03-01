export type OpenGatePluginConfig = {
  url: string;
  apiKey: string;
  agentId?: string;
  model?: string;
  pollIntervalMs?: number;
  maxConcurrent?: number;
};

export function resolveConfig(raw: Record<string, unknown>): OpenGatePluginConfig {
  const url = typeof raw.url === "string" ? raw.url.replace(/\/$/, "") : "";
  const apiKey = typeof raw.apiKey === "string" ? raw.apiKey : "";

  if (!url) throw new Error("[opengate] plugin config missing: url");
  if (!apiKey) throw new Error("[opengate] plugin config missing: apiKey");

  return {
    url,
    apiKey,
    agentId: typeof raw.agentId === "string" ? raw.agentId : "main",
    model: typeof raw.model === "string" ? raw.model : undefined,
    pollIntervalMs: typeof raw.pollIntervalMs === "number" ? raw.pollIntervalMs : 30_000,
    maxConcurrent: typeof raw.maxConcurrent === "number" ? raw.maxConcurrent : 3,
  };
}
