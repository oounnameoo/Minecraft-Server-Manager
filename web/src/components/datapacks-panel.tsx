import { useCallback, useEffect, useState } from 'react';
import {
  Check,
  Download,
  Globe,
  Link2,
  Loader2,
  Package,
  RefreshCw,
  Search,
  Trash2,
} from 'lucide-react';
import { ApiError } from '../api/client';
import { datapacksApi, type DatapackEntry } from '../api/datapacks';
import type { ModrinthSearchHit } from '../api/types';
import { usePermissions } from '../lib/auth';
import { formatBytes } from '../lib/formatters';
import { Badge } from './ui/badge';
import { Button } from './ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from './ui/card';
import { Input } from './ui/input';
import { useModal } from './ui/modal';

export function DatapacksPanel({
  serverId,
  worldName,
  mcVersion,
}: {
  serverId: string;
  worldName?: string;
  mcVersion?: string;
}) {
  const can = usePermissions();
  const canManage = can('worlds.manage', serverId) || can('files.write', serverId);
  const { confirm, dialog } = useModal();

  const [activeWorld, setActiveWorld] = useState(worldName ?? 'world');
  const [installed, setInstalled] = useState<DatapackEntry[]>([]);
  const [loadingInstalled, setLoadingInstalled] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const [query, setQuery] = useState('');
  const [matchVersion, setMatchVersion] = useState(false);
  const [results, setResults] = useState<ModrinthSearchHit[]>([]);
  const [searching, setSearching] = useState(false);
  const [installingId, setInstallingId] = useState<string | null>(null);

  const [customUrl, setCustomUrl] = useState('');
  const [installingUrl, setInstallingUrl] = useState(false);

  const loadInstalled = useCallback(async () => {
    setLoadingInstalled(true);
    try {
      const res = await datapacksApi.list(serverId, worldName);
      setActiveWorld(res.world);
      setInstalled(res.datapacks ?? []);
      setError(null);
    } catch (err) {
      setError(err instanceof ApiError ? err.detail : 'Failed to load datapacks');
    } finally {
      setLoadingInstalled(false);
    }
  }, [serverId, worldName]);

  const searchOnline = useCallback(
    async (q: string, filterByVer: boolean) => {
      setSearching(true);
      try {
        const res = await datapacksApi.searchModrinth(
          q,
          filterByVer ? mcVersion : undefined,
          15,
          0,
        );
        setResults(res.hits ?? []);
      } catch (err) {
        setError(err instanceof ApiError ? err.detail : 'Failed to search Modrinth datapacks');
      } finally {
        setSearching(false);
      }
    },
    [mcVersion],
  );

  useEffect(() => {
    void loadInstalled();
  }, [loadInstalled]);

  useEffect(() => {
    void searchOnline('', matchVersion);
  }, [searchOnline, matchVersion]);

  async function installFromModrinth(hit: ModrinthSearchHit) {
    setInstallingId(hit.project_id);
    setError(null);
    setNotice(null);
    try {
      let versions = await datapacksApi.modrinthVersions(
        hit.project_id,
        matchVersion ? mcVersion : undefined,
      );
      if (versions.length === 0 && matchVersion) {
        versions = await datapacksApi.modrinthVersions(hit.project_id);
      }
      const chosenVersion = versions[0];
      const file =
        chosenVersion?.files.find((f) => f.primary) ?? chosenVersion?.files[0];
      if (!file) {
        throw new Error('No downloadable file found for this datapack.');
      }
      const res = await datapacksApi.download(serverId, {
        world: activeWorld,
        url: file.url,
        filename: file.filename,
      });
      setNotice(`Installed ${hit.title} (${res.datapack.file}) into ${res.world}/datapacks. Run /reload or restart to apply.`);
      await loadInstalled();
    } catch (err) {
      setError(
        err instanceof ApiError
          ? err.detail
          : err instanceof Error
            ? err.message
            : 'Failed to install datapack',
      );
    } finally {
      setInstallingId(null);
    }
  }

  async function installFromUrl() {
    const trimmed = customUrl.trim();
    if (!trimmed) return;
    setInstallingUrl(true);
    setError(null);
    setNotice(null);
    try {
      const urlPath = trimmed.split('?')[0];
      const rawName = decodeURIComponent(urlPath.split('/').pop() || 'datapack.zip');
      const filename = rawName.endsWith('.zip') ? rawName : `${rawName}.zip`;
      const res = await datapacksApi.download(serverId, {
        world: activeWorld,
        url: trimmed,
        filename,
      });
      setCustomUrl('');
      setNotice(`Installed ${res.datapack.file} into ${res.world}/datapacks. Run /reload or restart to apply.`);
      await loadInstalled();
    } catch (err) {
      setError(err instanceof ApiError ? err.detail : 'Failed to download datapack from URL');
    } finally {
      setInstallingUrl(false);
    }
  }

  async function toggleDatapack(dp: DatapackEntry) {
    setError(null);
    setNotice(null);
    try {
      await datapacksApi.toggle(serverId, { world: activeWorld, file: dp.file });
      await loadInstalled();
    } catch (err) {
      setError(err instanceof ApiError ? err.detail : 'Failed to toggle datapack');
    }
  }

  async function deleteDatapack(dp: DatapackEntry) {
    if (
      !(await confirm(`Delete datapack "${dp.name}" from ${activeWorld}/datapacks?`, {
        title: 'Delete datapack',
        confirmLabel: 'Delete',
        destructive: true,
      }))
    ) {
      return;
    }
    setError(null);
    setNotice(null);
    try {
      await datapacksApi.remove(serverId, dp.file, activeWorld);
      setNotice(`Deleted ${dp.file}.`);
      await loadInstalled();
    } catch (err) {
      setError(err instanceof ApiError ? err.detail : 'Failed to delete datapack');
    }
  }

  return (
    <div className="space-y-4">
      {dialog}

      {notice && (
        <div className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 p-3 text-sm text-emerald-700 dark:text-emerald-300">
          {notice}
        </div>
      )}

      {error && (
        <div className="rounded-lg border border-destructive/40 bg-destructive/10 p-3 text-sm text-destructive">
          {error}
        </div>
      )}

      {/* Installed Datapacks */}
      <Card>
        <CardHeader className="pb-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div>
              <CardTitle className="text-base">
                Installed Datapacks ({activeWorld})
              </CardTitle>
              <CardDescription>
                Stored in <code className="font-mono text-xs">{activeWorld}/datapacks/</code>. Run{' '}
                <code className="font-mono text-xs">/reload</code> in-game or restart the server after changes.
              </CardDescription>
            </div>
            <Button
              size="sm"
              variant="outline"
              disabled={loadingInstalled}
              onClick={() => void loadInstalled()}
              className="gap-1.5"
            >
              <RefreshCw className={`h-3.5 w-3.5 ${loadingInstalled ? 'animate-spin' : ''}`} />
              Refresh
            </Button>
          </div>
        </CardHeader>
        <CardContent>
          {loadingInstalled ? (
            <p className="flex items-center gap-2 text-sm text-muted-foreground py-4">
              <Loader2 className="h-4 w-4 animate-spin" /> Loading datapacks…
            </p>
          ) : installed.length === 0 ? (
            <p className="text-sm text-muted-foreground py-2">
              No datapacks installed in {activeWorld} yet. Browse Modrinth or paste a direct .zip URL below.
            </p>
          ) : (
            <div className="divide-y rounded-lg border">
              {installed.map((dp) => (
                <div
                  key={dp.file}
                  className="flex flex-col gap-2 px-3 py-2.5 sm:flex-row sm:items-center sm:justify-between"
                >
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="truncate text-sm font-medium">{dp.name}</span>
                      <Badge
                        variant={dp.enabled ? 'secondary' : 'outline'}
                        className="text-[11px]"
                      >
                        {dp.enabled ? 'Enabled' : 'Disabled'}
                      </Badge>
                    </div>
                    <p className="truncate font-mono text-xs text-muted-foreground">
                      {dp.file}
                      {!dp.is_dir ? ` · ${formatBytes(dp.size_bytes)}` : ' · folder'}
                    </p>
                  </div>
                  {canManage && (
                    <div className="flex shrink-0 items-center gap-2">
                      {!dp.is_dir && (
                        <Button
                          size="sm"
                          variant="outline"
                          onClick={() => void toggleDatapack(dp)}
                          className="h-7 px-2.5 text-xs"
                        >
                          {dp.enabled ? 'Disable' : 'Enable'}
                        </Button>
                      )}
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => void deleteDatapack(dp)}
                        className="h-7 px-2 text-destructive hover:bg-destructive/10"
                        title="Delete datapack"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </Button>
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
        </CardContent>
      </Card>

      {/* Online Datapack Adder */}
      <Card>
        <CardHeader className="pb-3">
          <CardTitle className="flex items-center gap-2 text-base">
            <Globe className="h-4 w-4 text-primary" />
            Add Datapack from Online (Modrinth)
          </CardTitle>
          <CardDescription>
            Search and install datapacks directly from Modrinth or paste a direct .zip download URL.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void searchOnline(query, matchVersion);
            }}
            className="flex flex-wrap items-center gap-2"
          >
            <div className="relative min-w-[220px] flex-1">
              <Search className="pointer-events-none absolute left-3 top-2.5 h-4 w-4 text-muted-foreground" />
              <Input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Search Modrinth datapacks (e.g. Terralith, Incendium, Timber)..."
                className="pl-9"
              />
            </div>
            {mcVersion && (
              <Button
                type="button"
                size="sm"
                variant={matchVersion ? 'default' : 'outline'}
                onClick={() => setMatchVersion((v) => !v)}
                className="text-xs"
              >
                {matchVersion ? `MC ${mcVersion}` : 'All MC versions'}
              </Button>
            )}
            <Button type="submit" disabled={searching} className="gap-1.5">
              {searching ? <Loader2 className="h-4 w-4 animate-spin" /> : <Search className="h-4 w-4" />}
              Search
            </Button>
          </form>

          {/* Direct URL install */}
          {canManage && (
            <div className="flex flex-wrap items-center gap-2 rounded-lg border bg-muted/30 p-2.5">
              <Link2 className="h-4 w-4 shrink-0 text-muted-foreground" />
              <Input
                value={customUrl}
                onChange={(e) => setCustomUrl(e.target.value)}
                placeholder="Or paste a direct datapack .zip URL (https://...)"
                className="h-8 flex-1 min-w-[220px] text-xs"
              />
              <Button
                size="sm"
                variant="outline"
                disabled={installingUrl || !customUrl.trim()}
                onClick={() => void installFromUrl()}
                className="h-8 gap-1.5 text-xs"
              >
                {installingUrl ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <Download className="h-3.5 w-3.5" />
                )}
                Install URL
              </Button>
            </div>
          )}

          {/* Modrinth results */}
          {searching ? (
            <p className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin" /> Searching Modrinth…
            </p>
          ) : results.length === 0 ? (
            <p className="py-4 text-sm text-muted-foreground">
              No datapacks found. Try a different search term.
            </p>
          ) : (
            <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
              {results.map((hit) => {
                const slugMatch = installed.some((d) =>
                  d.name.toLowerCase().includes(hit.slug.toLowerCase()),
                );
                return (
                  <div
                    key={hit.project_id}
                    className="flex flex-col justify-between gap-3 rounded-lg border bg-card p-3"
                  >
                    <div className="flex items-start gap-3">
                      <div className="flex h-10 w-10 shrink-0 items-center justify-center overflow-hidden rounded-lg border bg-secondary/40">
                        {hit.icon_url ? (
                          <img
                            src={hit.icon_url}
                            alt=""
                            loading="lazy"
                            className="h-full w-full object-contain"
                          />
                        ) : (
                          <Package className="h-5 w-5 text-muted-foreground" />
                        )}
                      </div>
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center justify-between gap-2">
                          <h4 className="truncate text-sm font-semibold">{hit.title}</h4>
                          <span className="shrink-0 text-[11px] text-muted-foreground">
                            {hit.downloads.toLocaleString()} dl
                          </span>
                        </div>
                        <p className="text-xs text-muted-foreground">by {hit.author}</p>
                        <p className="mt-1 line-clamp-2 text-xs text-muted-foreground">
                          {hit.description}
                        </p>
                      </div>
                    </div>
                    <div className="flex items-center justify-end gap-2 border-t pt-2">
                      {slugMatch && (
                        <Badge variant="secondary" className="gap-1 text-[11px]">
                          <Check className="h-3 w-3" /> Installed
                        </Badge>
                      )}
                      <Button
                        size="sm"
                        disabled={!canManage || installingId === hit.project_id}
                        onClick={() => void installFromModrinth(hit)}
                        className="h-7 gap-1.5 px-3 text-xs"
                      >
                        {installingId === hit.project_id ? (
                          <Loader2 className="h-3.5 w-3.5 animate-spin" />
                        ) : (
                          <Download className="h-3.5 w-3.5" />
                        )}
                        {installingId === hit.project_id ? 'Installing…' : 'Install'}
                      </Button>
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
