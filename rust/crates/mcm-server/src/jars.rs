use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use std::path::Path;

pub async fn download_server_jar(
    client: &reqwest::Client,
    software: &str,
    version: &str,
    dest_dir: &Path,
) -> Result<String> {
    let sw = software.trim().to_lowercase();
    if sw == "custom" {
        return Ok("server.jar".to_string());
    }

    std::fs::create_dir_all(dest_dir).context("create server directory")?;
    let jar_name = "server.jar".to_string();
    let dest_path = dest_dir.join(&jar_name);

    let url = match sw.as_str() {
        "paper" | "velocity" | "waterfall" => resolve_papermc_url(client, &sw, version).await?,
        "purpur" => format!("https://api.purpurmc.org/v2/purpur/{version}/latest/download"),
        "vanilla" => resolve_vanilla_url(client, version).await?,
        "fabric" => resolve_fabric_url(client, version).await?,
        other => return Err(anyhow!("unsupported server software: {other}")),
    };

    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("download {sw} {version} from {url}"))?;
    if !resp.status().is_success() {
        return Err(anyhow!(
            "upstream download failed ({}): {}",
            resp.status(),
            url
        ));
    }
    let bytes = resp.bytes().await?;
    tokio::fs::write(&dest_path, &bytes)
        .await
        .context("write server.jar")?;

    Ok(jar_name)
}

async fn resolve_papermc_url(
    client: &reqwest::Client,
    project: &str,
    version: &str,
) -> Result<String> {
    let builds_url =
        format!("https://api.papermc.io/v2/projects/{project}/versions/{version}/builds");
    let val: Value = client
        .get(&builds_url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let builds = val
        .get("builds")
        .and_then(|b| b.as_array())
        .ok_or_else(|| anyhow!("no builds found for {project} {version}"))?;
    let latest = builds
        .last()
        .ok_or_else(|| anyhow!("empty builds list for {project} {version}"))?;
    let build_num = latest
        .get("build")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| anyhow!("missing build number"))?;
    let download_name = latest
        .pointer("/downloads/application/name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing download name"))?;
    Ok(format!(
        "https://api.papermc.io/v2/projects/{project}/versions/{version}/builds/{build_num}/downloads/{download_name}"
    ))
}

async fn resolve_vanilla_url(client: &reqwest::Client, version: &str) -> Result<String> {
    let manifest: Value = client
        .get("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let versions = manifest
        .get("versions")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("invalid mojang manifest"))?;
    let entry = versions
        .iter()
        .find(|v| v.get("id").and_then(|id| id.as_str()) == Some(version))
        .ok_or_else(|| anyhow!("vanilla version {version} not found"))?;
    let meta_url = entry
        .get("url")
        .and_then(|u| u.as_str())
        .ok_or_else(|| anyhow!("missing version meta url"))?;
    let meta: Value = client
        .get(meta_url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let server_url = meta
        .pointer("/downloads/server/url")
        .and_then(|u| u.as_str())
        .ok_or_else(|| anyhow!("missing server download url for {version}"))?;
    Ok(server_url.to_string())
}

async fn resolve_fabric_url(client: &reqwest::Client, version: &str) -> Result<String> {
    let loaders: Value = client
        .get("https://meta.fabricmc.net/v2/versions/loader")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let loader = loaders
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("version"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("no fabric loader version"))?;

    let installers: Value = client
        .get("https://meta.fabricmc.net/v2/versions/installer")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let installer = installers
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("version"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("no fabric installer version"))?;

    Ok(format!(
        "https://meta.fabricmc.net/v2/versions/loader/{version}/{loader}/{installer}/server/jar"
    ))
}

pub async fn list_versions(client: &reqwest::Client, software: &str) -> Result<Vec<String>> {
    let sw = software.trim().to_lowercase();
    match sw.as_str() {
        "paper" | "velocity" | "waterfall" => {
            let val: Value = client
                .get(format!("https://api.papermc.io/v2/projects/{sw}"))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let mut versions: Vec<String> = val
                .get("versions")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            versions.reverse();
            Ok(versions)
        }
        "purpur" => {
            let val: Value = client
                .get("https://api.purpurmc.org/v2/purpur")
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let mut versions: Vec<String> = val
                .get("versions")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            versions.reverse();
            Ok(versions)
        }
        "vanilla" => {
            let val: Value = client
                .get("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json")
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            Ok(val
                .get("versions")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("release"))
                        .filter_map(|v| v.get("id").and_then(|id| id.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default())
        }
        "fabric" => {
            let val: Value = client
                .get("https://meta.fabricmc.net/v2/versions/game")
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            Ok(val
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter(|v| v.get("stable").and_then(|s| s.as_bool()) == Some(true))
                        .filter_map(|v| {
                            v.get("version")
                                .and_then(|id| id.as_str())
                                .map(String::from)
                        })
                        .collect()
                })
                .unwrap_or_default())
        }
        _ => Ok(vec![]),
    }
}
