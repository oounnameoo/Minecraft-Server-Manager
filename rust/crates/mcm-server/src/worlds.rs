use crate::auth::User;
use crate::error::ApiError;
use crate::servers::{ServerStore, check_server_perm};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct WorldDimension {
    pub id: String,
    pub name: String,
    pub regions: usize,
}

#[derive(Serialize)]
pub struct WorldInfo {
    pub name: String,
    pub active: bool,
    pub generated: bool,
    pub seed: Option<String>,
    pub level_type: String,
    pub generate_structures: bool,
    pub size_bytes: u64,
    pub modified_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub metadata_error: bool,
    pub has_level_dat: bool,
    pub dimensions: Vec<WorldDimension>,
}

#[derive(Default)]
struct LevelDatMeta {
    seed: Option<String>,
    version: Option<String>,
    generate_structures: Option<bool>,
    level_type: Option<String>,
}

struct NbtCursor<'a>(&'a [u8]);

impl<'a> NbtCursor<'a> {
    fn read_u8(&mut self) -> Option<u8> {
        let (&b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(b)
    }

    fn read_u16(&mut self) -> Option<u16> {
        if self.0.len() < 2 {
            return None;
        }
        let (head, rest) = self.0.split_at(2);
        self.0 = rest;
        Some(u16::from_be_bytes([head[0], head[1]]))
    }

    fn read_i32(&mut self) -> Option<i32> {
        if self.0.len() < 4 {
            return None;
        }
        let (head, rest) = self.0.split_at(4);
        self.0 = rest;
        Some(i32::from_be_bytes([head[0], head[1], head[2], head[3]]))
    }

    fn read_i64(&mut self) -> Option<i64> {
        if self.0.len() < 8 {
            return None;
        }
        let (head, rest) = self.0.split_at(8);
        self.0 = rest;
        Some(i64::from_be_bytes([
            head[0], head[1], head[2], head[3], head[4], head[5], head[6], head[7],
        ]))
    }

    fn skip(&mut self, n: usize) -> Option<()> {
        if self.0.len() < n {
            return None;
        }
        self.0 = &self.0[n..];
        Some(())
    }

    fn read_str(&mut self) -> Option<String> {
        let len = self.read_u16()? as usize;
        if self.0.len() < len {
            return None;
        }
        let (head, rest) = self.0.split_at(len);
        self.0 = rest;
        Some(String::from_utf8_lossy(head).to_string())
    }

    fn skip_payload(&mut self, tag: u8) -> Option<()> {
        match tag {
            1 => self.skip(1),
            2 => self.skip(2),
            3 | 5 => self.skip(4),
            4 | 6 => self.skip(8),
            7 => {
                let len = self.read_i32()?.max(0) as usize;
                self.skip(len)
            }
            8 => {
                let len = self.read_u16()? as usize;
                self.skip(len)
            }
            9 => {
                let elem_tag = self.read_u8()?;
                let count = self.read_i32()?.max(0) as usize;
                for _ in 0..count {
                    self.skip_payload(elem_tag)?;
                }
                Some(())
            }
            10 => loop {
                let child_tag = self.read_u8()?;
                if child_tag == 0 {
                    break Some(());
                }
                let _ = self.read_str()?;
                self.skip_payload(child_tag)?;
            },
            11 => {
                let count = self.read_i32()?.max(0) as usize;
                self.skip(count.saturating_mul(4))
            }
            12 => {
                let count = self.read_i32()?.max(0) as usize;
                self.skip(count.saturating_mul(8))
            }
            _ => None,
        }
    }
}

fn walk_compound(cur: &mut NbtCursor<'_>, path: &str, out: &mut LevelDatMeta) -> Option<()> {
    loop {
        let tag = cur.read_u8()?;
        if tag == 0 {
            return Some(());
        }
        let name = cur.read_str()?;
        let full = if path.is_empty() {
            name
        } else {
            format!("{path}.{name}")
        };
        match (tag, full.as_str()) {
            (4, "Data.WorldGenSettings.seed") | (4, "Data.RandomSeed") | (4, "data.seed") => {
                let seed = cur.read_i64()?;
                if out.seed.is_none() || full != "Data.RandomSeed" {
                    out.seed = Some(seed.to_string());
                }
            }
            (8, "Data.Version.Name") => {
                out.version = Some(cur.read_str()?);
            }
            (1, "Data.WorldGenSettings.generate_features")
            | (1, "Data.MapFeatures")
            | (1, "data.generate_structures") => {
                let b = cur.read_u8()?;
                if out.generate_structures.is_none() || full != "Data.MapFeatures" {
                    out.generate_structures = Some(b != 0);
                }
            }
            (8, "Data.WorldGenSettings.dimensions.minecraft:overworld.generator.type")
            | (8, "data.dimensions.minecraft:overworld.generator.type")
            | (8, "Data.generatorName") => {
                let lt = cur.read_str()?;
                if out.level_type.is_none() || full != "Data.generatorName" {
                    out.level_type = Some(lt);
                }
            }
            (10, _) => walk_compound(cur, &full, out)?,
            _ => cur.skip_payload(tag)?,
        }
    }
}

fn read_nbt_meta_into(path: &std::path::Path, meta: &mut LevelDatMeta) -> Result<(), ()> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|_| ())?;
    let mut gz = flate2::read::GzDecoder::new(file);
    let mut buf = Vec::new();
    gz.read_to_end(&mut buf).map_err(|_| ())?;
    let mut cur = NbtCursor(&buf);
    let root_tag = cur.read_u8().ok_or(())?;
    if root_tag != 10 {
        return Err(());
    }
    let _ = cur.read_str().ok_or(())?;
    let _ = walk_compound(&mut cur, "", meta);
    Ok(())
}

fn read_level_dat_meta(world_dir: &std::path::Path) -> Result<LevelDatMeta, ()> {
    let mut meta = LevelDatMeta::default();
    read_nbt_meta_into(&world_dir.join("level.dat"), &mut meta)?;
    let wgs =
        world_dir.join("dimensions/minecraft/overworld/data/minecraft/world_gen_settings.dat");
    if wgs.is_file() {
        let _ = read_nbt_meta_into(&wgs, &mut meta);
    }
    Ok(meta)
}

fn count_mca_files(dir: &std::path::Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .filter(|e| {
            e.path().is_file() && e.file_name().to_str().is_some_and(|n| n.ends_with(".mca"))
        })
        .count()
}

fn read_active_world(root: &std::path::Path) -> String {
    if let Ok(props) = std::fs::read_to_string(root.join("server.properties")) {
        for line in props.lines() {
            if let Some(val) = line.strip_prefix("level-name=") {
                let trimmed = val.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
        }
    }
    "world".to_string()
}

pub(crate) fn dir_size(path: &std::path::Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in rd.flatten() {
        if let Ok(meta) = entry.metadata() {
            if meta.is_dir() {
                total += dir_size(&entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

pub async fn handle_list_worlds(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "worlds.read")?;
    let _ = store.get(&id)?;
    let root = store.server_dir(&id);
    let active_world = read_active_world(&root);
    let editable = !store.runtime.is_running(&id).await;
    let mut worlds = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("level.dat").exists() {
                let name = entry.file_name().to_string_lossy().to_string();
                let modified_at = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
                    .unwrap_or_default();

                let mut dimensions = Vec::new();
                let overworld_new = path.join("dimensions/minecraft/overworld");
                if overworld_new.join("region").is_dir() {
                    dimensions.push(WorldDimension {
                        id: format!("{name}/dimensions/minecraft/overworld"),
                        name: "Overworld".to_string(),
                        regions: count_mca_files(&overworld_new.join("region")),
                    });
                } else {
                    dimensions.push(WorldDimension {
                        id: name.clone(),
                        name: "Overworld".to_string(),
                        regions: count_mca_files(&path.join("region")),
                    });
                }

                let nether_new = path.join("dimensions/minecraft/the_nether");
                let nether_vanilla = path.join("DIM-1");
                let nether_paper = root.join(format!("{name}_nether")).join("DIM-1");
                if nether_new.join("region").is_dir() {
                    dimensions.push(WorldDimension {
                        id: format!("{name}/dimensions/minecraft/the_nether"),
                        name: "Nether".to_string(),
                        regions: count_mca_files(&nether_new.join("region")),
                    });
                } else if nether_vanilla.is_dir() {
                    dimensions.push(WorldDimension {
                        id: format!("{name}/DIM-1"),
                        name: "Nether".to_string(),
                        regions: count_mca_files(&nether_vanilla.join("region")),
                    });
                } else if nether_paper.is_dir() {
                    dimensions.push(WorldDimension {
                        id: format!("{name}_nether/DIM-1"),
                        name: "Nether".to_string(),
                        regions: count_mca_files(&nether_paper.join("region")),
                    });
                }

                let end_new = path.join("dimensions/minecraft/the_end");
                let end_vanilla = path.join("DIM1");
                let end_paper = root.join(format!("{name}_the_end")).join("DIM1");
                if end_new.join("region").is_dir() {
                    dimensions.push(WorldDimension {
                        id: format!("{name}/dimensions/minecraft/the_end"),
                        name: "The End".to_string(),
                        regions: count_mca_files(&end_new.join("region")),
                    });
                } else if end_vanilla.is_dir() {
                    dimensions.push(WorldDimension {
                        id: format!("{name}/DIM1"),
                        name: "The End".to_string(),
                        regions: count_mca_files(&end_vanilla.join("region")),
                    });
                } else if end_paper.is_dir() {
                    dimensions.push(WorldDimension {
                        id: format!("{name}_the_end/DIM1"),
                        name: "The End".to_string(),
                        regions: count_mca_files(&end_paper.join("region")),
                    });
                }

                let (meta, metadata_error) = match read_level_dat_meta(&path) {
                    Ok(m) => (m, false),
                    Err(()) => (LevelDatMeta::default(), true),
                };

                worlds.push(WorldInfo {
                    active: name == active_world,
                    name,
                    generated: true,
                    seed: meta.seed,
                    level_type: meta
                        .level_type
                        .unwrap_or_else(|| "minecraft:normal".to_string()),
                    generate_structures: meta.generate_structures.unwrap_or(true),
                    size_bytes: dir_size(&path),
                    modified_at,
                    version: meta.version,
                    metadata_error,
                    has_level_dat: true,
                    dimensions,
                });
            }
        }
    }
    Ok(Json(serde_json::json!({
        "worlds": worlds,
        "active_world": active_world,
        "editable": editable,
    })))
}

pub async fn handle_delete_world(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path((id, name)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "worlds.manage")?;
    let _ = store.get(&id)?;
    if name.contains('/') || name.contains("..") {
        return Err(ApiError::bad_request("invalid world name"));
    }
    let path = store.server_dir(&id).join(&name);
    if !path.join("level.dat").exists() {
        return Err(ApiError::not_found("world not found"));
    }
    tokio::fs::remove_dir_all(&path)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn resolve_dimension_region_dir(
    root: &std::path::Path,
    world_name: &str,
    dimension: &str,
) -> Result<std::path::PathBuf, ApiError> {
    if world_name.contains('/') || world_name.contains("..") || dimension.contains("..") {
        return Err(ApiError::bad_request("invalid world or dimension"));
    }
    let dim = dimension.trim_matches('/');
    if dim.is_empty() || dim == world_name {
        let legacy = root.join(world_name).join("region");
        if legacy.is_dir() {
            return Ok(legacy);
        }
        let modern = root
            .join(world_name)
            .join("dimensions/minecraft/overworld/region");
        if modern.is_dir() {
            return Ok(modern);
        }
        return Ok(legacy);
    }
    let rel = std::path::Path::new(dim);
    for comp in rel.components() {
        if !matches!(comp, std::path::Component::Normal(_)) {
            return Err(ApiError::bad_request("invalid dimension path"));
        }
    }
    Ok(root.join(rel).join("region"))
}

fn parse_mca_coords(name: &str) -> Option<(i32, i32)> {
    let inner = name.strip_prefix("r.")?.strip_suffix(".mca")?;
    let (xs, zs) = inner.split_once('.')?;
    Some((xs.parse().ok()?, zs.parse().ok()?))
}

fn count_mca_chunks(path: &std::path::Path) -> usize {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return 0;
    };
    let mut header = [0u8; 4096];
    if file.read_exact(&mut header).is_err() {
        return 0;
    }
    header
        .chunks_exact(4)
        .filter(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]) != 0)
        .count()
}

#[derive(Deserialize)]
pub struct RegionsQuery {
    #[serde(default)]
    pub dimension: String,
    #[serde(default)]
    pub offset: usize,
}

pub async fn handle_world_regions(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path((id, name)): Path<(String, String)>,
    Query(q): Query<RegionsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "worlds.read")?;
    let _ = store.get(&id)?;
    let region_dir = resolve_dimension_region_dir(&store.server_dir(&id), &name, &q.dimension)?;
    let mut all = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&region_dir) {
        for entry in rd.flatten() {
            let fname = entry.file_name().to_string_lossy().to_string();
            let Some((x, z)) = parse_mca_coords(&fname) else {
                continue;
            };
            let size_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let chunks = count_mca_chunks(&entry.path());
            all.push(serde_json::json!({
                "x": x,
                "z": z,
                "chunks": chunks,
                "size_bytes": size_bytes,
            }));
        }
    }
    all.sort_by_key(|r| {
        (
            r.get("x").and_then(|v| v.as_i64()).unwrap_or(0).abs()
                + r.get("z").and_then(|v| v.as_i64()).unwrap_or(0).abs(),
            r.get("x").and_then(|v| v.as_i64()).unwrap_or(0),
            r.get("z").and_then(|v| v.as_i64()).unwrap_or(0),
        )
    });
    let total = all.len();
    let offset = q.offset.min(total);
    let regions: Vec<_> = all.into_iter().skip(offset).take(200).collect();
    Ok(Json(serde_json::json!({
        "regions": regions,
        "total": total,
        "offset": offset,
    })))
}

#[derive(Deserialize)]
pub struct ChunksQuery {
    #[serde(default)]
    pub dimension: String,
    pub x: i32,
    pub z: i32,
}

pub async fn handle_world_chunks(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path((id, name)): Path<(String, String)>,
    Query(q): Query<ChunksQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "worlds.read")?;
    let _ = store.get(&id)?;
    let region_dir = resolve_dimension_region_dir(&store.server_dir(&id), &name, &q.dimension)?;
    let mca_path = region_dir.join(format!("r.{}.{}.mca", q.x, q.z));
    if !mca_path.is_file() {
        return Ok(Json(serde_json::json!({ "chunks": [] })));
    }
    let raw = tokio::fs::read(&mca_path)
        .await
        .map_err(|e| ApiError::internal(format!("read region: {e}")))?;
    let mut chunks = Vec::new();
    if raw.len() >= 8192 {
        for i in 0..1024usize {
            let loc =
                u32::from_be_bytes([raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2], raw[i * 4 + 3]]);
            if loc == 0 {
                continue;
            }
            let ts_off = 4096 + i * 4;
            let timestamp = u32::from_be_bytes([
                raw[ts_off],
                raw[ts_off + 1],
                raw[ts_off + 2],
                raw[ts_off + 3],
            ]);
            let cx = q.x * 32 + (i as i32 % 32);
            let cz = q.z * 32 + (i as i32 / 32);
            chunks.push(serde_json::json!({
                "x": cx,
                "z": cz,
                "timestamp": timestamp,
            }));
        }
    }
    Ok(Json(serde_json::json!({ "chunks": chunks })))
}

#[derive(Deserialize)]
pub struct ChunkResetReq {
    #[serde(default)]
    pub dimension: String,
    pub region_x: i32,
    pub region_z: i32,
    pub chunks: Vec<usize>,
}

pub async fn handle_world_chunks_reset(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path((id, name)): Path<(String, String)>,
    Json(req): Json<ChunkResetReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "worlds.manage")?;
    let _ = store.get(&id)?;
    if store.runtime.is_running(&id).await {
        return Err(ApiError::conflict(
            "server_running",
            "Stop the server before resetting chunks.",
        ));
    }
    let region_dir = resolve_dimension_region_dir(&store.server_dir(&id), &name, &req.dimension)?;
    let mca_path = region_dir.join(format!("r.{}.{}.mca", req.region_x, req.region_z));
    if !mca_path.is_file() {
        return Err(ApiError::not_found("region file not found"));
    }
    let mut raw = tokio::fs::read(&mca_path)
        .await
        .map_err(|e| ApiError::internal(format!("read region: {e}")))?;
    if raw.len() < 8192 {
        return Err(ApiError::bad_request("invalid region header"));
    }
    let mut reset = 0usize;
    for idx in req.chunks {
        if idx < 1024 {
            let loc_off = idx * 4;
            let ts_off = 4096 + idx * 4;
            if raw[loc_off..loc_off + 4] != [0, 0, 0, 0] {
                raw[loc_off..loc_off + 4].fill(0);
                raw[ts_off..ts_off + 4].fill(0);
                reset += 1;
            }
        }
    }
    tokio::fs::write(&mca_path, &raw)
        .await
        .map_err(|e| ApiError::internal(format!("write region: {e}")))?;
    Ok(Json(serde_json::json!({
        "reset": reset,
        "recovery_path": "",
    })))
}
