import { request } from './client';
import type { ModrinthSearchHit, ModrinthVersion } from './types';

export interface DatapackEntry {
  name: string;
  file: string;
  size_bytes: number;
  enabled: boolean;
  is_dir: boolean;
  modified_at: string;
}

export interface DatapackList {
  world: string;
  datapacks: DatapackEntry[];
}

export interface ModrinthSearchResponse {
  hits: ModrinthSearchHit[];
  total_hits: number;
  offset: number;
  limit: number;
}

const base = (id: string) => `/api/servers/${encodeURIComponent(id)}/datapacks`;

export const datapacksApi = {
  list: (serverId: string, world?: string): Promise<DatapackList> => {
    const qs = world ? `?${new URLSearchParams({ world })}` : '';
    return request(`${base(serverId)}${qs}`);
  },
  download: (
    serverId: string,
    payload: { world?: string; url: string; filename: string },
  ): Promise<{ ok: boolean; world: string; datapack: DatapackEntry }> =>
    request(`${base(serverId)}/download`, {
      method: 'POST',
      body: JSON.stringify(payload),
    }),
  toggle: (
    serverId: string,
    payload: { world?: string; file: string },
  ): Promise<{ ok: boolean; file: string; enabled: boolean }> =>
    request(`${base(serverId)}/toggle`, {
      method: 'POST',
      body: JSON.stringify(payload),
    }),
  remove: (serverId: string, file: string, world?: string): Promise<{ ok: boolean }> => {
    const qs = world ? `?${new URLSearchParams({ world })}` : '';
    return request(`${base(serverId)}/${encodeURIComponent(file)}${qs}`, {
      method: 'DELETE',
    });
  },
  searchModrinth: async (
    query: string,
    mcVersion?: string,
    limit = 20,
    offset = 0,
  ): Promise<ModrinthSearchResponse> => {
    const facets: string[][] = [['project_type:datapack']];
    if (mcVersion?.trim()) {
      facets.push([`versions:${mcVersion.trim()}`]);
    }
    const params = new URLSearchParams({
      query: query.trim(),
      facets: JSON.stringify(facets),
      limit: String(limit),
      offset: String(offset),
      index: query.trim() ? 'relevance' : 'downloads',
    });
    const url = `https://api.modrinth.com/v2/search?${params.toString()}`;
    return request(`/api/proxy?${new URLSearchParams({ url }).toString()}`);
  },
  modrinthVersions: async (
    projectId: string,
    mcVersion?: string,
  ): Promise<ModrinthVersion[]> => {
    const params = new URLSearchParams();
    if (mcVersion?.trim()) {
      params.set('game_versions', JSON.stringify([mcVersion.trim()]));
    }
    const q = params.toString();
    const url = `https://api.modrinth.com/v2/project/${encodeURIComponent(projectId)}/version${q ? `?${q}` : ''}`;
    return request(`/api/proxy?${new URLSearchParams({ url }).toString()}`);
  },
};
