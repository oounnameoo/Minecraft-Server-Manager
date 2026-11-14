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
        format!("https://fill.papermc.io/v3/projects/{project}/versions/{version}/builds");
    let val: Value = client
        .get(&builds_url)
        .header("User-Agent", "mcm/0.1.0")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    extract_papermc_download_url(&val, project, version)
}

fn extract_papermc_download_url(val: &Value, project: &str, version: &str) -> Result<String> {
    let builds = val
        .as_array()
        .ok_or_else(|| anyhow!("no builds found for {project} {version}"))?;
    let selected = builds
        .iter()
        .find(|b| b.get("channel").and_then(|c| c.as_str()) == Some("STABLE"))
        .or_else(|| builds.first())
        .ok_or_else(|| anyhow!("empty builds list for {project} {version}"))?;
    selected
        .pointer("/downloads/server:default/url")
        .and_then(|u| u.as_str())
        .or_else(|| {
            selected
                .get("downloads")
                .and_then(|d| d.as_object())
                .and_then(|map| map.values().next())
                .and_then(|entry| entry.get("url"))
                .and_then(|u| u.as_str())
        })
        .map(String::from)
        .ok_or_else(|| anyhow!("missing download url for {project} {version}"))
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
                .get(format!("https://fill.papermc.io/v3/projects/{sw}"))
                .header("User-Agent", "mcm/0.1.0")
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            Ok(extract_papermc_versions(&val))
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

#[derive(serde::Serialize)]
pub struct JarBuildInfo {
    pub version: String,
    pub build: String,
    pub display: String,
}

pub async fn list_builds(
    client: &reqwest::Client,
    software: &str,
    version: &str,
) -> Result<Vec<JarBuildInfo>> {
    let sw = software.trim().to_lowercase();
    match sw.as_str() {
        "paper" | "velocity" | "waterfall" => {
            let val: Value = client
                .get(format!(
                    "https://fill.papermc.io/v3/projects/{sw}/versions/{version}/builds"
                ))
                .header("User-Agent", "mcm/0.1.0")
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let builds = val
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|b| {
                            let id = b.get("id")?.as_i64()?.to_string();
                            let channel = b.get("channel").and_then(|c| c.as_str()).unwrap_or("");
                            let display = if channel.is_empty() || channel == "STABLE" {
                                id.clone()
                            } else {
                                format!("{id} ({channel})")
                            };
                            Some(JarBuildInfo {
                                version: version.to_string(),
                                build: id,
                                display,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(builds)
        }
        "purpur" => {
            let val: Value = client
                .get(format!("https://api.purpurmc.org/v2/purpur/{version}"))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let mut builds: Vec<JarBuildInfo> = val
                .pointer("/builds/all")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|b| b.as_str())
                        .map(|b| JarBuildInfo {
                            version: version.to_string(),
                            build: b.to_string(),
                            display: b.to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            builds.reverse();
            Ok(builds)
        }
        "fabric" => {
            let val: Value = client
                .get("https://meta.fabricmc.net/v2/versions/loader")
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            Ok(val
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|b| b.get("version").and_then(|v| v.as_str()))
                        .take(25)
                        .map(|b| JarBuildInfo {
                            version: version.to_string(),
                            build: b.to_string(),
                            display: b.to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default())
        }
        _ => Ok(vec![JarBuildInfo {
            version: version.to_string(),
            build: "latest".to_string(),
            display: "latest".to_string(),
        }]),
    }
}

fn parse_version_group(group: &str) -> Vec<u32> {
    group
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<u32>().ok())
        .collect()
}

fn extract_papermc_versions(val: &Value) -> Vec<String> {
    if let Some(obj) = val.get("versions").and_then(|v| v.as_object()) {
        let mut groups: Vec<(&String, &Value)> = obj.iter().collect();
        groups.sort_by_key(|(b, _)| std::cmp::Reverse(parse_version_group(b)));
        let mut versions = Vec::new();
        for (_, list) in groups {
            if let Some(arr) = list.as_array() {
                for item in arr {
                    if let Some(ver) = item.as_str() {
                        versions.push(ver.to_string());
                    }
                }
            }
        }
        return versions;
    }

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
    versions
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_papermc_versions_orders_groups_newest_first() {
        let payload = json!({
            "project": { "id": "paper", "name": "Paper" },
            "versions": {
                "1.8": ["1.8.8"],
                "1.21": ["1.21.11", "1.21.4"],
                "26.1": ["26.1.2", "26.1.1"],
                "1.20": ["1.20.6", "1.20.4"]
            }
        });
        assert_eq!(
            extract_papermc_versions(&payload),
            vec!["26.1.2", "26.1.1", "1.21.11", "1.21.4", "1.20.6", "1.20.4", "1.8.8"]
        );
    }

    #[test]
    fn extract_papermc_download_url_prefers_stable_build() {
        let payload = json!([
            {
                "id": 233,
                "channel": "EXPERIMENTAL",
                "downloads": {
                    "server:default": {
                        "url": "https://fill-data.papermc.io/v1/objects/exp/paper-exp.jar"
                    }
                }
            },
            {
                "id": 232,
                "channel": "STABLE",
                "downloads": {
                    "server:default": {
                        "url": "https://fill-data.papermc.io/v1/objects/stable/paper-232.jar"
                    }
                }
            }
        ]);
        let url = extract_papermc_download_url(&payload, "paper", "1.21.4").unwrap();
        assert_eq!(
            url,
            "https://fill-data.papermc.io/v1/objects/stable/paper-232.jar"
        );
    }
}
