use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

pub type JarProgressFn = Arc<dyn Fn(u64, u64, &str) + Send + Sync>;

pub async fn download_server_jar(
    client: &reqwest::Client,
    software: &str,
    version: &str,
    dest_dir: &Path,
    progress: Option<JarProgressFn>,
) -> Result<String> {
    let sw = software.trim().to_lowercase();
    if sw == "custom" {
        return Ok("server.jar".to_string());
    }

    std::fs::create_dir_all(dest_dir).context("create server directory")?;
    if sw == "forge" {
        return install_forge_server(client, version, dest_dir, progress).await;
    }
    if sw == "neoforge" {
        return install_neoforge_server(client, version, dest_dir, progress).await;
    }
    if sw == "quilt" {
        return install_quilt_server(client, version, dest_dir, progress).await;
    }
    let jar_name = "server.jar".to_string();
    let dest_path = dest_dir.join(&jar_name);

    let stage = format!("Resolving {sw} {version}...");
    if let Some(cb) = &progress {
        cb(0, 0, &stage);
    }

    let url = match sw.as_str() {
        "paper" | "velocity" | "waterfall" => resolve_papermc_url(client, &sw, version).await?,
        "purpur" => format!("https://api.purpurmc.org/v2/purpur/{version}/latest/download"),
        "vanilla" => resolve_vanilla_url(client, version).await?,
        "fabric" => resolve_fabric_url(client, version).await?,
        other => return Err(anyhow!("unsupported server software: {other}")),
    };

    let dl_stage = format!("Downloading {sw} {version}...");
    download_to_file_with_progress(client, &url, &dest_path, &dl_stage, progress)
        .await
        .with_context(|| format!("download {sw} {version} from {url}"))?;

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
        "quilt" => {
            let val: Value = client
                .get("https://meta.quiltmc.org/v3/versions/game")
                .header("User-Agent", "mcm/0.1.0")
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
        "forge" => {
            let val: Value = client
                .get("https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json")
                .header("User-Agent", "mcm/0.1.0")
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let mut set = std::collections::BTreeSet::new();
            if let Some(promos) = val.get("promos").and_then(|p| p.as_object()) {
                for key in promos.keys() {
                    if let Some(ver) = key
                        .strip_suffix("-recommended")
                        .or_else(|| key.strip_suffix("-latest"))
                    {
                        set.insert(ver.to_string());
                    }
                }
            }
            let mut versions: Vec<String> = set.into_iter().collect();
            versions.sort_by_key(|v| std::cmp::Reverse(parse_version_group(v)));
            Ok(versions)
        }
        "neoforge" => {
            let xml = client
                .get("https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml")
                .header("User-Agent", "mcm/0.1.0")
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            let mut set = std::collections::BTreeSet::new();
            for nf in extract_xml_versions(&xml) {
                if let Some(mc) = neoforge_to_mc_version(&nf) {
                    set.insert(mc);
                }
            }
            let mut versions: Vec<String> = set.into_iter().collect();
            versions.sort_by_key(|v| std::cmp::Reverse(parse_version_group(v)));
            Ok(versions)
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
        "quilt" => {
            let val: Value = client
                .get("https://meta.quiltmc.org/v3/versions/loader")
                .header("User-Agent", "mcm/0.1.0")
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
        "forge" => {
            let val: Value = client
                .get("https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json")
                .header("User-Agent", "mcm/0.1.0")
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let mut builds = Vec::new();
            if let Some(promos) = val.get("promos").and_then(|p| p.as_object()) {
                for suffix in ["recommended", "latest"] {
                    let key = format!("{version}-{suffix}");
                    if let Some(fv) = promos.get(&key).and_then(|v| v.as_str()) {
                        let fv_str = fv.to_string();
                        if !builds.iter().any(|b: &JarBuildInfo| b.build == fv_str) {
                            builds.push(JarBuildInfo {
                                version: version.to_string(),
                                build: fv_str.clone(),
                                display: format!("{fv_str} ({suffix})"),
                            });
                        }
                    }
                }
            }
            Ok(builds)
        }
        "neoforge" => {
            let xml = client
                .get("https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml")
                .header("User-Agent", "mcm/0.1.0")
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            let prefix = mc_to_neoforge_prefix(version);
            let mut builds: Vec<JarBuildInfo> = extract_xml_versions(&xml)
                .into_iter()
                .filter(|v| v.starts_with(&prefix))
                .map(|v| JarBuildInfo {
                    version: version.to_string(),
                    build: v.clone(),
                    display: v,
                })
                .collect();
            builds.reverse();
            builds.truncate(25);
            Ok(builds)
        }
        _ => Ok(vec![JarBuildInfo {
            version: version.to_string(),
            build: "latest".to_string(),
            display: "latest".to_string(),
        }]),
    }
}

async fn run_java_installer(dest_dir: &Path, installer_name: &str, args: &[&str]) -> Result<()> {
    let out = tokio::process::Command::new("java")
        .arg("-jar")
        .arg(installer_name)
        .args(args)
        .current_dir(dest_dir)
        .output()
        .await
        .with_context(|| format!("run java -jar {installer_name}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        return Err(anyhow!(
            "{installer_name} failed ({}): {} {}",
            out.status,
            stdout.trim(),
            stderr.trim()
        ));
    }
    Ok(())
}

async fn download_to_file_with_progress(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    stage: &str,
    progress: Option<JarProgressFn>,
) -> Result<()> {
    let mut resp = client
        .get(url)
        .header("User-Agent", "mcm/0.1.0")
        .send()
        .await
        .with_context(|| format!("download {url}"))?;
    if !resp.status().is_success() {
        return Err(anyhow!(
            "upstream download failed ({}): {}",
            resp.status(),
            url
        ));
    }
    let total = resp.content_length().unwrap_or(0);
    if let Some(cb) = &progress {
        cb(0, total, stage);
    }
    let mut file = tokio::fs::File::create(dest)
        .await
        .context("create destination file")?;
    let mut loaded: u64 = 0;
    let mut last_report: u64 = 0;
    while let Some(chunk) = resp.chunk().await? {
        file.write_all(&chunk).await?;
        loaded += chunk.len() as u64;
        if loaded - last_report >= 131_072 {
            last_report = loaded;
            if let Some(cb) = &progress {
                cb(loaded, total, stage);
            }
        }
    }
    file.flush().await?;
    if let Some(cb) = &progress {
        cb(loaded, total.max(loaded), stage);
    }
    Ok(())
}

async fn install_forge_server(
    client: &reqwest::Client,
    version: &str,
    dest_dir: &Path,
    progress: Option<JarProgressFn>,
) -> Result<String> {
    if let Some(cb) = &progress {
        cb(0, 0, &format!("Resolving Forge {version}..."));
    }
    let val: Value = client
        .get("https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json")
        .header("User-Agent", "mcm/0.1.0")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let promos = val
        .get("promos")
        .and_then(|p| p.as_object())
        .ok_or_else(|| anyhow!("invalid forge promotions response"))?;
    let forge_ver = promos
        .get(&format!("{version}-recommended"))
        .or_else(|| promos.get(&format!("{version}-latest")))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("no forge build found for Minecraft {version}"))?;

    let url = format!(
        "https://maven.minecraftforge.net/net/minecraftforge/forge/{version}-{forge_ver}/forge-{version}-{forge_ver}-installer.jar"
    );
    let installer_path = dest_dir.join("forge-installer.jar");
    download_to_file_with_progress(
        client,
        &url,
        &installer_path,
        &format!("Downloading Forge {version}-{forge_ver} installer..."),
        progress.clone(),
    )
    .await?;
    if let Some(cb) = &progress {
        cb(0, 0, "Running Forge server installer...");
    }
    run_java_installer(dest_dir, "forge-installer.jar", &["--installServer"]).await?;
    let _ = std::fs::remove_file(&installer_path);
    let _ = std::fs::remove_file(dest_dir.join("forge-installer.jar.log"));

    for candidate in [
        format!("forge-{version}-{forge_ver}.jar"),
        format!("forge-{version}-{forge_ver}-universal.jar"),
    ] {
        let p = dest_dir.join(&candidate);
        if p.is_file() {
            let _ = std::fs::rename(&p, dest_dir.join("server.jar"));
            break;
        }
    }
    Ok("server.jar".to_string())
}

async fn install_neoforge_server(
    client: &reqwest::Client,
    version: &str,
    dest_dir: &Path,
    progress: Option<JarProgressFn>,
) -> Result<String> {
    if let Some(cb) = &progress {
        cb(0, 0, &format!("Resolving NeoForge {version}..."));
    }
    let xml = client
        .get("https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml")
        .header("User-Agent", "mcm/0.1.0")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let prefix = mc_to_neoforge_prefix(version);
    let matching: Vec<String> = extract_xml_versions(&xml)
        .into_iter()
        .filter(|v| v.starts_with(&prefix))
        .collect();
    let nf_ver = matching
        .iter()
        .rfind(|v| !v.contains("-beta"))
        .or_else(|| matching.last())
        .ok_or_else(|| anyhow!("no neoforge build found for Minecraft {version}"))?;

    let url = format!(
        "https://maven.neoforged.net/releases/net/neoforged/neoforge/{nf_ver}/neoforge-{nf_ver}-installer.jar"
    );
    let installer_path = dest_dir.join("neoforge-installer.jar");
    download_to_file_with_progress(
        client,
        &url,
        &installer_path,
        &format!("Downloading NeoForge {nf_ver} installer..."),
        progress.clone(),
    )
    .await?;
    if let Some(cb) = &progress {
        cb(0, 0, "Running NeoForge server installer...");
    }
    run_java_installer(dest_dir, "neoforge-installer.jar", &["--installServer"]).await?;
    let _ = std::fs::remove_file(&installer_path);
    let _ = std::fs::remove_file(dest_dir.join("neoforge-installer.jar.log"));
    Ok("server.jar".to_string())
}

async fn install_quilt_server(
    client: &reqwest::Client,
    version: &str,
    dest_dir: &Path,
    progress: Option<JarProgressFn>,
) -> Result<String> {
    if let Some(cb) = &progress {
        cb(0, 0, &format!("Resolving Quilt {version}..."));
    }
    let installers: Value = client
        .get("https://meta.quiltmc.org/v3/versions/installer")
        .header("User-Agent", "mcm/0.1.0")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let url = installers
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("url"))
        .and_then(|u| u.as_str())
        .ok_or_else(|| anyhow!("no quilt installer found"))?;
    let installer_path = dest_dir.join("quilt-installer.jar");
    download_to_file_with_progress(
        client,
        url,
        &installer_path,
        "Downloading Quilt installer...",
        progress.clone(),
    )
    .await?;
    if let Some(cb) = &progress {
        cb(0, 0, "Running Quilt server installer...");
    }
    run_java_installer(
        dest_dir,
        "quilt-installer.jar",
        &[
            "install",
            "server",
            version,
            "--download-server",
            "--install-dir=.",
        ],
    )
    .await?;
    let _ = std::fs::remove_file(&installer_path);
    let launch_jar = dest_dir.join("quilt-server-launch.jar");
    if launch_jar.is_file() {
        let _ = std::fs::rename(&launch_jar, dest_dir.join("server.jar"));
    }
    Ok("server.jar".to_string())
}

fn extract_xml_versions(xml: &str) -> Vec<String> {
    let mut versions = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<version>") {
        let after = &rest[start + "<version>".len()..];
        if let Some(end) = after.find("</version>") {
            let v = after[..end].trim();
            if !v.is_empty() {
                versions.push(v.to_string());
            }
            rest = &after[end + "</version>".len()..];
        } else {
            break;
        }
    }
    versions
}

fn mc_to_neoforge_prefix(mc: &str) -> String {
    let parts: Vec<&str> = mc.trim().split('.').collect();
    if parts.first() == Some(&"1") && parts.len() >= 2 {
        let minor = parts[1];
        let patch = parts.get(2).copied().unwrap_or("0");
        return format!("{minor}.{patch}.");
    }
    format!("{}.", mc.trim())
}

fn neoforge_to_mc_version(nf: &str) -> Option<String> {
    let parts: Vec<&str> = nf.split('.').collect();
    if parts.len() < 3 {
        return None;
    }
    let major: u32 = parts[0].parse().ok()?;
    if major >= 26 {
        let third = parts[2];
        if third == "0" {
            Some(format!("{}.{}", parts[0], parts[1]))
        } else {
            Some(format!("{}.{}.{}", parts[0], parts[1], third))
        }
    } else if parts[1] == "0" {
        Some(format!("1.{}", parts[0]))
    } else {
        Some(format!("1.{}.{}", parts[0], parts[1]))
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
            vec![
                "26.1.2", "26.1.1", "1.21.11", "1.21.4", "1.20.6", "1.20.4", "1.8.8"
            ]
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
