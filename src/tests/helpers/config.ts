import type { ConfigDomain, ConfigSnapshot, ConfigUpdate, Settings } from '../../core/config/generated/config';
import { SETTINGS_DEFAULTS } from '../../core/config/generated/defaults';
import { applySettingsFields, cloneConfig, type SettingsFieldValues } from '../../core/config/fields';

export function makeSettings(changes: Partial<SettingsFieldValues> = {}): Settings {
  return applySettingsFields(SETTINGS_DEFAULTS, changes);
}

export function makeConfigSnapshot(data: Settings = makeSettings(), revision = '1'): ConfigSnapshot {
  return {
    data: cloneConfig(data),
    domains: Object.fromEntries(Object.keys(data).map(domain => [domain, {
      revision, schemaVersion: 1,
    }])) as unknown as ConfigSnapshot['domains'],
  };
}

export function configRequestChanges(request: ConfigUpdate): Partial<SettingsFieldValues> {
  return Object.fromEntries(request.patches.flatMap(patch =>
    Object.entries(patch.changes).map(([path, value]) => [`${patch.domain}.${path}`, value]),
  )) as Partial<SettingsFieldValues>;
}

/** An accepting IPC double. Validation/conflict tests provide explicit rejected/conflict results. */
export function applyConfigRequest(snapshot: ConfigSnapshot, request: ConfigUpdate): ConfigSnapshot {
  const result = cloneConfig(snapshot);
  result.data = applySettingsFields(result.data, configRequestChanges(request));
  for (const patch of request.patches) {
    const domain = patch.domain as ConfigDomain;
    result.domains[domain].revision = String(BigInt(result.domains[domain].revision) + 1n);
  }
  return result;
}
