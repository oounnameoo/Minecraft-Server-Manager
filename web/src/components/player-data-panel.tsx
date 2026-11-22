import { useCallback, useEffect, useMemo, useState } from 'react';
import { Database, Loader2, RefreshCw, Search, ShieldCheck } from 'lucide-react';
import { api, ApiError } from '../api/client';
import { nbtApi, type PlayerDataFile } from '../api/nbt';
import { usePermissions } from '../lib/auth';
import { NBTEditor } from './nbt-editor';
import { Badge } from './ui/badge';
import { Button } from './ui/button';
import { Input } from './ui/input';

export function PlayerDataPanel({
  serverId,
  state,
  onDirtyChange,
}: {
  serverId: string;
  state: string;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const can = usePermissions();
  const canWhitelist = can('whitelist.manage', serverId);
  const [players, setPlayers] = useState<PlayerDataFile[]>([]);
  const [whitelistedNames, setWhitelistedNames] = useState<Set<string>>(new Set());
  const [whitelistingName, setWhitelistingName] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(true);
  const [query, setQuery] = useState('');
  const [limit, setLimit] = useState(50);
  const load = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const [playersRes, whitelistRes] = await Promise.all([
        nbtApi.players(serverId),
        api.whitelist(serverId).catch(() => ({ whitelist: [] })),
      ]);
      setPlayers(playersRes.players);
      setWhitelistedNames(
        new Set((whitelistRes.whitelist ?? []).map((w) => w.name.toLowerCase())),
      );
    } catch (err) {
      setError(err instanceof ApiError ? err.detail : 'Could not load saved player data. Try again.');
    } finally {
      setBusy(false);
    }
  }, [serverId]);
  useEffect(() => {
    void load();
  }, [load]);

  async function addToWhitelist(playerName: string) {
    setWhitelistingName(playerName);
    setError(null);
    setNotice(null);
    try {
      const res = await api.addWhitelist(serverId, playerName);
      if (res.whitelist) {
        setWhitelistedNames(new Set(res.whitelist.map((w) => w.name.toLowerCase())));
      } else {
        setWhitelistedNames((prev) => new Set([...prev, playerName.toLowerCase()]));
      }
      setNotice(`Added ${playerName} to whitelist.`);
    } catch (err) {
      setError(err instanceof ApiError ? err.detail : `Failed to whitelist ${playerName}.`);
    } finally {
      setWhitelistingName(null);
    }
  }
  const filtered = useMemo(
    () =>
      players.filter((p) =>
        `${p.name} ${p.uuid} ${p.world}`.toLowerCase().includes(query.trim().toLowerCase()),
      ),
    [players, query],
  );
  if (selected)
    return (
      <NBTEditor
        key={selected}
        serverId={serverId}
        path={selected}
        state={state}
        onDirtyChange={onDirtyChange}
        onClose={() => setSelected(null)}
      />
    );
  return (
    <section aria-label="Saved player data" className="min-w-0 space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h3 className="section-heading">Player data</h3>
          <p className="mt-1 max-w-prose text-sm text-muted-foreground">
            Edit saved health, food, XP, position and inventory. Players are listed from world playerdata
            folders, including offline players. Open other NBT files from Files.
          </p>
        </div>
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void load()}>
          <RefreshCw className="mr-2 h-4 w-4" />
          Refresh player data
        </Button>
      </div>
      <div className="relative max-w-md">
        <Search className="pointer-events-none absolute left-3 top-3 h-4 w-4 text-muted-foreground" />
        <Input
          aria-label="Search saved players"
          placeholder="Search player name, UUID or world"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setLimit(50);
          }}
          className="pl-9"
        />
      </div>
      {notice && (
        <p role="status" className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 p-3 text-sm text-emerald-700 dark:text-emerald-300">
          {notice}
        </p>
      )}
      {error && (
        <p role="alert" className="rounded-lg border border-destructive/40 p-3 text-sm">
          {error}
        </p>
      )}
      {busy ? (
        <p role="status" className="flex items-center gap-2 py-8 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          Finding player files…
        </p>
      ) : (
        !error && (
          <>
            <p role="status" className="text-xs text-muted-foreground">
              {filtered.length} player {filtered.length === 1 ? 'file' : 'files'}
            </p>
            <div className="divide-y rounded-lg border bg-card">
              {filtered.length === 0 ? (
                <div className="empty-state">
                  <Database />
                  <h4 className="section-heading">
                    {query ? 'No matching players' : 'No saved player data'}
                  </h4>
                  <p>
                    {query
                      ? 'Try a different name, UUID or world.'
                      : 'Player files appear after a player joins a Java Edition world and the server saves. You can browse other NBT files in Files.'}
                  </p>
                </div>
              ) : (
                filtered.slice(0, limit).map((p) => {
                  const isWhitelisted = !!p.name && whitelistedNames.has(p.name.toLowerCase());
                  return (
                    <div
                      key={p.path}
                      className="flex flex-col gap-2 px-4 py-3 hover:bg-accent/40 sm:flex-row sm:items-center sm:justify-between"
                    >
                      <button
                        type="button"
                        onClick={() => setSelected(p.path)}
                        className="flex min-w-0 flex-1 flex-col gap-0.5 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring rounded"
                      >
                        <span className="block break-all text-sm font-medium">{p.name || p.uuid}</span>
                        <span className="block break-all font-mono text-xs text-muted-foreground">
                          {p.name ? `${p.uuid} · ` : ''}
                          {p.world}
                        </span>
                      </button>
                      <div className="flex shrink-0 items-center gap-2">
                        {canWhitelist && p.name && (
                          isWhitelisted ? (
                            <Badge variant="secondary" className="gap-1 text-xs text-emerald-600 dark:text-emerald-400">
                              <ShieldCheck className="h-3 w-3" /> Whitelisted
                            </Badge>
                          ) : (
                            <Button
                              size="sm"
                              variant="outline"
                              disabled={whitelistingName === p.name}
                              onClick={() => void addToWhitelist(p.name)}
                              className="h-7 px-2.5 text-xs gap-1"
                            >
                              <ShieldCheck className="h-3.5 w-3.5" />
                              {whitelistingName === p.name ? 'Adding…' : 'Add to whitelist'}
                            </Button>
                          )
                        )}
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => setSelected(p.path)}
                          className="h-7 px-2.5 text-xs text-primary"
                        >
                          Open player data
                        </Button>
                      </div>
                    </div>
                  );
                })
              )}
            </div>
            {filtered.length > limit && (
              <Button size="sm" variant="outline" onClick={() => setLimit((n) => n + 50)}>
                Show more players
              </Button>
            )}
          </>
        )
      )}
    </section>
  );
}
