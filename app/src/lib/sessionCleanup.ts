import type { Overview } from "../api";

export function withoutClearedSessions(overview: Overview, cleared: ReadonlyMap<string, ReadonlySet<string>>): Overview {
  if (!cleared.size) return overview;
  return {
    ...overview,
    devices: overview.devices.map((device) => {
      const ids = cleared.get(device.endpoint_id);
      return ids ? { ...device, sessions: device.sessions?.filter((session) => !ids.has(session.id)) } : device;
    }),
  };
}
