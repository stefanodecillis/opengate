import fs from "node:fs";
import path from "node:path";

/**
 * Persistent state to track which tasks have already been spawned.
 * Stored as a JSON file in the plugin state directory.
 * Prevents double-spawning if the poll fires before a session completes.
 */

type StateEntry = {
  taskId: string;
  sessionKey: string;
  spawnedAt: number;
};

type StateFile = {
  spawned: Record<string, StateEntry>;
};

const TTL_MS = 24 * 60 * 60 * 1000; // 1 day — clean up old entries

export class TaskState {
  private filePath: string;
  private data: StateFile;

  constructor(stateDir: string) {
    this.filePath = path.join(stateDir, "opengate-spawned.json");
    this.data = this.load();
    this.cleanup();
  }

  private load(): StateFile {
    try {
      const raw = fs.readFileSync(this.filePath, "utf-8");
      return JSON.parse(raw) as StateFile;
    } catch {
      return { spawned: {} };
    }
  }

  private save(): void {
    try {
      fs.mkdirSync(path.dirname(this.filePath), { recursive: true });
      fs.writeFileSync(this.filePath, JSON.stringify(this.data, null, 2), "utf-8");
    } catch (e) {
      console.error("[opengate] failed to save state:", e);
    }
  }

  private cleanup(): void {
    const cutoff = Date.now() - TTL_MS;
    let changed = false;
    for (const [id, entry] of Object.entries(this.data.spawned)) {
      if (entry.spawnedAt < cutoff) {
        delete this.data.spawned[id];
        changed = true;
      }
    }
    if (changed) this.save();
  }

  isSpawned(taskId: string): boolean {
    return taskId in this.data.spawned;
  }

  markSpawned(taskId: string, sessionKey: string): void {
    this.data.spawned[taskId] = { taskId, sessionKey, spawnedAt: Date.now() };
    this.save();
  }

  remove(taskId: string): void {
    delete this.data.spawned[taskId];
    this.save();
  }

  activeCount(): number {
    return Object.keys(this.data.spawned).length;
  }
}
