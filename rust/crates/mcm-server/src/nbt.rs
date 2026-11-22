use crate::auth::User;
use crate::error::ApiError;
use crate::files::{PathQuery, safe_resolve};
use crate::servers::{ServerStore, check_server_perm};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use flate2::{
    Compression,
    read::{GzDecoder, ZlibDecoder},
    write::{GzEncoder, ZlibEncoder},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NbtTag {
    #[serde(rename = "type")]
    pub tag_type: u8,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<NbtTag>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_type: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct NbtFile {
    pub path: String,
    pub revision: String,
    pub compression: String,
    pub root: NbtTag,
    pub editable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WriteNbtReq {
    pub path: String,
    pub expected_revision: String,
    pub root: NbtTag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NbtCompression {
    Gzip,
    Zlib,
    None,
}

impl NbtCompression {
    fn as_str(self) -> &'static str {
        match self {
            Self::Gzip => "gzip",
            Self::Zlib => "zlib",
            Self::None => "none",
        }
    }
}

fn detect_compression(raw: &[u8]) -> NbtCompression {
    if raw.len() >= 2 && raw[0] == 0x1f && raw[1] == 0x8b {
        NbtCompression::Gzip
    } else if raw.len() >= 2 && raw[0] == 0x78 && matches!(raw[1], 0x01 | 0x5e | 0x9c | 0xda) {
        NbtCompression::Zlib
    } else {
        NbtCompression::None
    }
}

fn decompress_nbt(raw: &[u8]) -> Result<(Vec<u8>, NbtCompression), ApiError> {
    let comp = detect_compression(raw);
    let mut out = Vec::new();
    match comp {
        NbtCompression::Gzip => {
            GzDecoder::new(raw)
                .read_to_end(&mut out)
                .map_err(|_| ApiError::bad_request("invalid gzip NBT data"))?;
        }
        NbtCompression::Zlib => {
            ZlibDecoder::new(raw)
                .read_to_end(&mut out)
                .map_err(|_| ApiError::bad_request("invalid zlib NBT data"))?;
        }
        NbtCompression::None => out.extend_from_slice(raw),
    }
    Ok((out, comp))
}

fn compress_nbt(raw_nbt: &[u8], comp: NbtCompression) -> Result<Vec<u8>, ApiError> {
    match comp {
        NbtCompression::Gzip => {
            let mut enc = GzEncoder::new(Vec::new(), Compression::default());
            enc.write_all(raw_nbt)
                .map_err(|_| ApiError::internal("failed to compress NBT"))?;
            enc.finish()
                .map_err(|_| ApiError::internal("failed to finish gzip NBT"))
        }
        NbtCompression::Zlib => {
            let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
            enc.write_all(raw_nbt)
                .map_err(|_| ApiError::internal("failed to compress NBT"))?;
            enc.finish()
                .map_err(|_| ApiError::internal("failed to finish zlib NBT"))
        }
        NbtCompression::None => Ok(raw_nbt.to_vec()),
    }
}

fn file_revision(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], ApiError> {
        if self.0.len() < n {
            return Err(ApiError::bad_request("unexpected end of NBT data"));
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }

    fn read_u8(&mut self) -> Result<u8, ApiError> {
        Ok(self.read_bytes(1)?[0])
    }

    fn read_i8(&mut self) -> Result<i8, ApiError> {
        Ok(self.read_u8()? as i8)
    }

    fn read_i16(&mut self) -> Result<i16, ApiError> {
        Ok(i16::from_be_bytes(self.read_bytes(2)?.try_into().unwrap()))
    }

    fn read_u16(&mut self) -> Result<u16, ApiError> {
        Ok(u16::from_be_bytes(self.read_bytes(2)?.try_into().unwrap()))
    }

    fn read_i32(&mut self) -> Result<i32, ApiError> {
        Ok(i32::from_be_bytes(self.read_bytes(4)?.try_into().unwrap()))
    }

    fn read_i64(&mut self) -> Result<i64, ApiError> {
        Ok(i64::from_be_bytes(self.read_bytes(8)?.try_into().unwrap()))
    }

    fn read_f32(&mut self) -> Result<f32, ApiError> {
        Ok(f32::from_be_bytes(self.read_bytes(4)?.try_into().unwrap()))
    }

    fn read_f64(&mut self) -> Result<f64, ApiError> {
        Ok(f64::from_be_bytes(self.read_bytes(8)?.try_into().unwrap()))
    }

    fn read_str(&mut self) -> Result<String, ApiError> {
        let len = self.read_u16()? as usize;
        let bytes = self.read_bytes(len)?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }
}

fn read_payload(cur: &mut Cursor<'_>, tag_type: u8, name: String) -> Result<NbtTag, ApiError> {
    Ok(match tag_type {
        1 => NbtTag {
            tag_type,
            name,
            value: Some(cur.read_i8()?.to_string()),
            children: None,
            list_type: None,
            values: None,
        },
        2 => NbtTag {
            tag_type,
            name,
            value: Some(cur.read_i16()?.to_string()),
            children: None,
            list_type: None,
            values: None,
        },
        3 => NbtTag {
            tag_type,
            name,
            value: Some(cur.read_i32()?.to_string()),
            children: None,
            list_type: None,
            values: None,
        },
        4 => NbtTag {
            tag_type,
            name,
            value: Some(cur.read_i64()?.to_string()),
            children: None,
            list_type: None,
            values: None,
        },
        5 => NbtTag {
            tag_type,
            name,
            value: Some(cur.read_f32()?.to_string()),
            children: None,
            list_type: None,
            values: None,
        },
        6 => NbtTag {
            tag_type,
            name,
            value: Some(cur.read_f64()?.to_string()),
            children: None,
            list_type: None,
            values: None,
        },
        7 => {
            let len = cur.read_i32()?;
            if len < 0 {
                return Err(ApiError::bad_request("negative NBT byte array length"));
            }
            let mut vals = Vec::with_capacity((len as usize).min(1024));
            for _ in 0..len {
                vals.push(cur.read_i8()?.to_string());
            }
            NbtTag {
                tag_type,
                name,
                value: None,
                children: None,
                list_type: None,
                values: Some(vals),
            }
        }
        8 => NbtTag {
            tag_type,
            name,
            value: Some(cur.read_str()?),
            children: None,
            list_type: None,
            values: None,
        },
        9 => {
            let list_type = cur.read_u8()?;
            let len = cur.read_i32()?;
            if len < 0 {
                return Err(ApiError::bad_request("negative NBT list length"));
            }
            let mut children = Vec::with_capacity((len as usize).min(1024));
            for _ in 0..len {
                children.push(read_payload(cur, list_type, String::new())?);
            }
            NbtTag {
                tag_type,
                name,
                value: None,
                children: Some(children),
                list_type: Some(list_type),
                values: None,
            }
        }
        10 => {
            let mut children = Vec::new();
            loop {
                let child_type = cur.read_u8()?;
                if child_type == 0 {
                    break;
                }
                let child_name = cur.read_str()?;
                children.push(read_payload(cur, child_type, child_name)?);
            }
            NbtTag {
                tag_type,
                name,
                value: None,
                children: Some(children),
                list_type: None,
                values: None,
            }
        }
        11 => {
            let len = cur.read_i32()?;
            if len < 0 {
                return Err(ApiError::bad_request("negative NBT int array length"));
            }
            let mut vals = Vec::with_capacity((len as usize).min(1024));
            for _ in 0..len {
                vals.push(cur.read_i32()?.to_string());
            }
            NbtTag {
                tag_type,
                name,
                value: None,
                children: None,
                list_type: None,
                values: Some(vals),
            }
        }
        12 => {
            let len = cur.read_i32()?;
            if len < 0 {
                return Err(ApiError::bad_request("negative NBT long array length"));
            }
            let mut vals = Vec::with_capacity((len as usize).min(1024));
            for _ in 0..len {
                vals.push(cur.read_i64()?.to_string());
            }
            NbtTag {
                tag_type,
                name,
                value: None,
                children: None,
                list_type: None,
                values: Some(vals),
            }
        }
        other => {
            return Err(ApiError::bad_request(format!(
                "unsupported NBT tag type {other}"
            )));
        }
    })
}

fn decode_nbt(bytes: &[u8]) -> Result<NbtTag, ApiError> {
    let mut cur = Cursor(bytes);
    let root_type = cur.read_u8()?;
    if root_type != 10 {
        return Err(ApiError::bad_request("NBT root must be a compound tag"));
    }
    let root_name = cur.read_str()?;
    read_payload(&mut cur, root_type, root_name)
}

fn write_str(out: &mut Vec<u8>, s: &str) -> Result<(), ApiError> {
    let bytes = s.as_bytes();
    let len = u16::try_from(bytes.len())
        .map_err(|_| ApiError::bad_request("NBT string exceeds u16 length"))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

fn write_payload(out: &mut Vec<u8>, tag: &NbtTag) -> Result<(), ApiError> {
    let val = tag.value.as_deref().unwrap_or("");
    match tag.tag_type {
        1 => {
            let v: i8 = val
                .parse()
                .map_err(|_| ApiError::bad_request(format!("invalid byte value: {val}")))?;
            out.push(v as u8);
        }
        2 => {
            let v: i16 = val
                .parse()
                .map_err(|_| ApiError::bad_request(format!("invalid short value: {val}")))?;
            out.extend_from_slice(&v.to_be_bytes());
        }
        3 => {
            let v: i32 = val
                .parse()
                .map_err(|_| ApiError::bad_request(format!("invalid int value: {val}")))?;
            out.extend_from_slice(&v.to_be_bytes());
        }
        4 => {
            let v: i64 = val
                .parse()
                .map_err(|_| ApiError::bad_request(format!("invalid long value: {val}")))?;
            out.extend_from_slice(&v.to_be_bytes());
        }
        5 => {
            let v: f32 = val
                .parse()
                .map_err(|_| ApiError::bad_request(format!("invalid float value: {val}")))?;
            out.extend_from_slice(&v.to_be_bytes());
        }
        6 => {
            let v: f64 = val
                .parse()
                .map_err(|_| ApiError::bad_request(format!("invalid double value: {val}")))?;
            out.extend_from_slice(&v.to_be_bytes());
        }
        7 => {
            let vals = tag.values.as_deref().unwrap_or(&[]);
            let len = i32::try_from(vals.len())
                .map_err(|_| ApiError::bad_request("byte array too large"))?;
            out.extend_from_slice(&len.to_be_bytes());
            for s in vals {
                let v: i8 = s
                    .parse()
                    .map_err(|_| ApiError::bad_request(format!("invalid byte array item: {s}")))?;
                out.push(v as u8);
            }
        }
        8 => write_str(out, val)?,
        9 => {
            let children = tag.children.as_deref().unwrap_or(&[]);
            let list_type = tag
                .list_type
                .unwrap_or_else(|| children.first().map(|c| c.tag_type).unwrap_or(0));
            out.push(list_type);
            let len = i32::try_from(children.len())
                .map_err(|_| ApiError::bad_request("NBT list too large"))?;
            out.extend_from_slice(&len.to_be_bytes());
            for child in children {
                write_payload(out, child)?;
            }
        }
        10 => {
            for child in tag.children.as_deref().unwrap_or(&[]) {
                out.push(child.tag_type);
                write_str(out, &child.name)?;
                write_payload(out, child)?;
            }
            out.push(0);
        }
        11 => {
            let vals = tag.values.as_deref().unwrap_or(&[]);
            let len = i32::try_from(vals.len())
                .map_err(|_| ApiError::bad_request("int array too large"))?;
            out.extend_from_slice(&len.to_be_bytes());
            for s in vals {
                let v: i32 = s
                    .parse()
                    .map_err(|_| ApiError::bad_request(format!("invalid int array item: {s}")))?;
                out.extend_from_slice(&v.to_be_bytes());
            }
        }
        12 => {
            let vals = tag.values.as_deref().unwrap_or(&[]);
            let len = i32::try_from(vals.len())
                .map_err(|_| ApiError::bad_request("long array too large"))?;
            out.extend_from_slice(&len.to_be_bytes());
            for s in vals {
                let v: i64 = s
                    .parse()
                    .map_err(|_| ApiError::bad_request(format!("invalid long array item: {s}")))?;
                out.extend_from_slice(&v.to_be_bytes());
            }
        }
        other => {
            return Err(ApiError::bad_request(format!(
                "unsupported NBT tag type {other}"
            )));
        }
    }
    Ok(())
}

fn encode_nbt(root: &NbtTag) -> Result<Vec<u8>, ApiError> {
    if root.tag_type != 10 {
        return Err(ApiError::bad_request("NBT root must be a compound tag"));
    }
    let mut out = Vec::new();
    out.push(root.tag_type);
    write_str(&mut out, &root.name)?;
    write_payload(&mut out, root)?;
    Ok(out)
}

pub async fn handle_read_nbt(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "files.read")?;
    let _ = store.get(&id)?;
    let root_dir = store.server_dir(&id);
    let target = safe_resolve(&root_dir, &q.path)?;
    let raw = tokio::fs::read(&target)
        .await
        .map_err(|_| ApiError::not_found("file not found"))?;
    let revision = file_revision(&raw);
    let (decompressed, comp) = decompress_nbt(&raw)?;
    let root = decode_nbt(&decompressed)?;
    let editable = !store.runtime.is_running(&id).await;
    Ok(Json(NbtFile {
        path: q.path,
        revision,
        compression: comp.as_str().to_string(),
        root,
        editable,
        backup_path: None,
    }))
}

pub async fn handle_write_nbt(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<WriteNbtReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "files.write")?;
    let _ = store.get(&id)?;
    if store.runtime.is_running(&id).await {
        return Err(ApiError::conflict(
            "server_running",
            "stop the server before editing NBT files",
        ));
    }
    let root_dir = store.server_dir(&id);
    let target = safe_resolve(&root_dir, &req.path)?;
    let existing = tokio::fs::read(&target)
        .await
        .map_err(|_| ApiError::not_found("file not found"))?;
    let current_rev = file_revision(&existing);
    if current_rev != req.expected_revision {
        return Err(ApiError::conflict(
            "revision_mismatch",
            "file changed on server",
        ));
    }
    let comp = detect_compression(&existing);
    let encoded = encode_nbt(&req.root)?;
    let compressed = compress_nbt(&encoded, comp)?;

    let backup_rel = format!("{}.bak", req.path);
    let backup_target = safe_resolve(&root_dir, &backup_rel)?;
    let _ = tokio::fs::write(&backup_target, &existing).await;

    let tmp_target = target.with_extension("nbt.tmp");
    tokio::fs::write(&tmp_target, &compressed)
        .await
        .map_err(|e| ApiError::internal(format!("write failed: {e}")))?;
    tokio::fs::rename(&tmp_target, &target)
        .await
        .map_err(|e| ApiError::internal(format!("rename failed: {e}")))?;

    let new_rev = file_revision(&compressed);
    Ok(Json(NbtFile {
        path: req.path,
        revision: new_rev,
        compression: comp.as_str().to_string(),
        root: req.root,
        editable: true,
        backup_path: Some(backup_rel),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nbt_round_trip_all_supported_tags() {
        let root = NbtTag {
            tag_type: 10,
            name: String::new(),
            value: None,
            children: Some(vec![
                NbtTag {
                    tag_type: 1,
                    name: "b".into(),
                    value: Some("-5".into()),
                    children: None,
                    list_type: None,
                    values: None,
                },
                NbtTag {
                    tag_type: 3,
                    name: "i".into(),
                    value: Some("42".into()),
                    children: None,
                    list_type: None,
                    values: None,
                },
                NbtTag {
                    tag_type: 4,
                    name: "l".into(),
                    value: Some("7881675549943722714".into()),
                    children: None,
                    list_type: None,
                    values: None,
                },
                NbtTag {
                    tag_type: 5,
                    name: "f".into(),
                    value: Some("1.5".into()),
                    children: None,
                    list_type: None,
                    values: None,
                },
                NbtTag {
                    tag_type: 8,
                    name: "s".into(),
                    value: Some("minecraft:overworld".into()),
                    children: None,
                    list_type: None,
                    values: None,
                },
                NbtTag {
                    tag_type: 9,
                    name: "Pos".into(),
                    value: None,
                    children: Some(vec![NbtTag {
                        tag_type: 6,
                        name: String::new(),
                        value: Some("128.5".into()),
                        children: None,
                        list_type: None,
                        values: None,
                    }]),
                    list_type: Some(6),
                    values: None,
                },
                NbtTag {
                    tag_type: 11,
                    name: "UUID".into(),
                    value: None,
                    children: None,
                    list_type: None,
                    values: Some(vec!["1".into(), "-2".into(), "3".into(), "4".into()]),
                },
            ]),
            list_type: None,
            values: None,
        };
        let raw = encode_nbt(&root).unwrap();
        let gz = compress_nbt(&raw, NbtCompression::Gzip).unwrap();
        let (decompressed, comp) = decompress_nbt(&gz).unwrap();
        assert_eq!(comp, NbtCompression::Gzip);
        let decoded = decode_nbt(&decompressed).unwrap();
        assert_eq!(decoded, root);
    }
}
