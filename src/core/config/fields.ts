import type { ConfigDomain, ConfigSnapshot, ConfigUpdate, DomainPatch, Settings } from './generated/config';

type LeafPaths<T> = {
  [K in keyof T & string]: T[K] extends readonly unknown[] ? K
    : T[K] extends object ? `${K}.${LeafPaths<T[K]>}` : K
}[keyof T & string];
type PathValue<T, P extends string> = P extends `${infer K}.${infer Rest}`
  ? K extends keyof T ? PathValue<T[K], Rest> : never
  : P extends keyof T ? T[P] : never;

export type SettingsFieldPath = LeafPaths<Settings>;
export type SettingsFieldValue<P extends SettingsFieldPath> = PathValue<Settings, P>;
export type SettingsFieldValues = { [P in SettingsFieldPath]: SettingsFieldValue<P> };

export function readSetting<P extends SettingsFieldPath>(settings: Settings, path: P): SettingsFieldValue<P> {
  return path.split('.').reduce<unknown>((value, key) => (value as Record<string, unknown>)[key], settings) as SettingsFieldValue<P>;
}

export function cloneConfig<T>(value: T): T { return JSON.parse(JSON.stringify(value)) as T; }
export function sameConfigValue(left: unknown, right: unknown): boolean { return JSON.stringify(left) === JSON.stringify(right); }

/** Values are captured by the caller; later changes must not rewrite queued intent. */
export function buildSettingsUpdate(
  snapshot: ConfigSnapshot,
  changes: Partial<SettingsFieldValues>,
  baseValues: Partial<SettingsFieldValues> = {},
  revisions: Partial<Record<ConfigDomain, string>> = {},
): ConfigUpdate {
  const patches = new Map<ConfigDomain, DomainPatch>();
  for (const key of Object.keys(changes) as SettingsFieldPath[]) {
    const value = changes[key];
    if (value === undefined) continue;
    const [domainName, ...parts] = key.split('.');
    const domain = domainName as ConfigDomain;
    let patch = patches.get(domain);
    if (!patch) {
      patch = { domain, expectedRevision: revisions[domain] ?? snapshot.domains[domain].revision, changes: {}, baseValues: {}, repair: false };
      patches.set(domain, patch);
    }
    const field = parts.join('.');
    patch.changes[field] = cloneConfig(value);
    patch.baseValues[field] = cloneConfig(baseValues[key] ?? readSetting(snapshot.data, key));
  }
  return { patches: [...patches.values()] };
}

export function applySettingsFields(settings: Settings, values: Partial<SettingsFieldValues>): Settings {
  const result = cloneConfig(settings);
  for (const [path, value] of Object.entries(values)) {
    const parts = path.split('.');
    const leaf = parts.pop()!;
    const target = parts.reduce<unknown>((node, key) => (node as Record<string, unknown>)[key], result) as Record<string, unknown>;
    target[leaf] = cloneConfig(value);
  }
  return result;
}
