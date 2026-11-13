use anyhow::{Context, Result};
use bollard::Docker;
use bollard::container::{
    AttachContainerOptions, Config as ContainerConfig, CreateContainerOptions,
    KillContainerOptions, LogsOptions, RemoveContainerOptions, StartContainerOptions, StatsOptions,
    StopContainerOptions,
};
use bollard::models::{HostConfig, PortBinding, RestartPolicy, RestartPolicyNameEnum};
use futures_util::StreamExt;
use serde::Serialize;
use std::collections::HashMap;
use tokio::io::AsyncWriteExt;

pub const DEFAULT_ENTRY_SCRIPT: &str = r#"#!/bin/sh
set -e
cd /data

if [ ! -f /data/eula.txt ]; then
  echo "eula=true" > /data/eula.txt
fi

if [ -n "${CUSTOM_START_CMD:-}" ]; then
  exec sh -c "$CUSTOM_START_CMD"
fi

SERVER_JAR="${SERVER_JAR:-server.jar}"
RAM="${SERVER_RAM_MB:-1024}"
JVM_FLAGS="${JVM_FLAGS:--XX:+UseG1GC -XX:+ParallelRefProcEnabled -XX:MaxGCPauseMillis=200}"

exec java -Xms"${RAM}M" -Xmx"${RAM}M" ${JVM_FLAGS} -jar "${SERVER_JAR}" nogui
"#;

#[derive(Clone)]
pub struct DockerClient {
    docker: Docker,
}

#[derive(Debug, Clone, Default)]
pub struct CreateOpts {
    pub name: String,
    pub image: String,
    pub data_dir_host: String,
    pub host_port: u16,
    pub container_port: u16,
    pub ram_mb: i64,
    pub cpu_limit: f64,
    pub memory_limit_mb: i64,
    pub jvm_flags: String,
    pub start_command: String,
    pub java_version: String,
    pub env_vars: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerStats {
    pub cpu_percent: f64,
    pub memory_used_mb: f64,
    pub memory_limit_mb: f64,
    pub memory_percent: f64,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
}

impl DockerClient {
    pub fn connect(docker_host: &str) -> Result<Self> {
        let docker = if let Some(sock) = docker_host.strip_prefix("unix://") {
            Docker::connect_with_unix(sock, 120, bollard::API_DEFAULT_VERSION)
                .context("connect to docker unix socket")?
        } else if docker_host.starts_with("tcp://") || docker_host.starts_with("http://") {
            Docker::connect_with_http(docker_host, 120, bollard::API_DEFAULT_VERSION)
                .context("connect to docker http")?
        } else {
            Docker::connect_with_socket_defaults().context("connect to docker defaults")?
        };
        Ok(Self { docker })
    }

    pub async fn ping(&self) -> Result<String> {
        let info = self.docker.version().await.context("docker version")?;
        Ok(info.version.unwrap_or_else(|| "connected".to_string()))
    }

    pub async fn create_container(&self, opts: &CreateOpts) -> Result<String> {
        let container_name = format!("mcm-{}", opts.name);
        let cport = if opts.container_port == 0 {
            25565
        } else {
            opts.container_port
        };
        let port_key = format!("{cport}/tcp");

        let mut exposed = HashMap::new();
        exposed.insert(port_key.clone(), HashMap::new());

        let mut port_bindings = HashMap::new();
        if opts.host_port > 0 {
            port_bindings.insert(
                port_key,
                Some(vec![PortBinding {
                    host_ip: Some("0.0.0.0".to_string()),
                    host_port: Some(opts.host_port.to_string()),
                }]),
            );
        }

        let mut env = vec![
            format!("SERVER_RAM_MB={}", opts.ram_mb),
            format!("JVM_FLAGS={}", opts.jvm_flags),
            format!("CUSTOM_START_CMD={}", opts.start_command),
            "SERVER_JAR=server.jar".to_string(),
            "EULA=TRUE".to_string(),
        ];
        if !opts.java_version.is_empty() {
            env.push(format!("JAVA_VERSION={}", opts.java_version));
        }
        for (k, v) in &opts.env_vars {
            env.push(format!("{k}={v}"));
        }

        let nano_cpus = if opts.cpu_limit > 0.0 {
            Some((opts.cpu_limit * 1_000_000_000.0) as i64)
        } else {
            None
        };
        let memory = if opts.memory_limit_mb > 0 {
            Some(opts.memory_limit_mb * 1024 * 1024)
        } else {
            None
        };

        let host_config = HostConfig {
            binds: Some(vec![format!("{}:/data", opts.data_dir_host)]),
            port_bindings: Some(port_bindings),
            restart_policy: Some(RestartPolicy {
                name: Some(RestartPolicyNameEnum::NO),
                maximum_retry_count: None,
            }),
            nano_cpus,
            memory,
            ..Default::default()
        };

        let config = ContainerConfig {
            image: Some(opts.image.clone()),
            cmd: Some(vec![
                "sh".to_string(),
                "-c".to_string(),
                DEFAULT_ENTRY_SCRIPT.to_string(),
            ]),
            env: Some(env),
            exposed_ports: Some(exposed),
            open_stdin: Some(true),
            tty: Some(false),
            host_config: Some(host_config),
            ..Default::default()
        };

        let res = self
            .docker
            .create_container(
                Some(CreateContainerOptions {
                    name: container_name.as_str(),
                    platform: None,
                }),
                config,
            )
            .await
            .context("docker create container")?;

        Ok(res.id)
    }

    pub async fn start_container(&self, container_id: &str) -> Result<()> {
        self.docker
            .start_container(container_id, None::<StartContainerOptions<String>>)
            .await
            .context("docker start container")
    }

    pub async fn stop_container(&self, container_id: &str, timeout_secs: i64) -> Result<()> {
        self.docker
            .stop_container(container_id, Some(StopContainerOptions { t: timeout_secs }))
            .await
            .context("docker stop container")
    }

    pub async fn kill_container(&self, container_id: &str) -> Result<()> {
        self.docker
            .kill_container(
                container_id,
                Some(KillContainerOptions { signal: "SIGKILL" }),
            )
            .await
            .context("docker kill container")
    }

    pub async fn remove_container(&self, container_id: &str) -> Result<()> {
        let _ = self
            .docker
            .remove_container(
                container_id,
                Some(RemoveContainerOptions {
                    force: true,
                    v: false,
                    ..Default::default()
                }),
            )
            .await;
        Ok(())
    }

    pub async fn inspect_status(&self, container_id: &str) -> Option<String> {
        let info = self
            .docker
            .inspect_container(container_id, None)
            .await
            .ok()?;
        let state = info.state?;
        if state.running == Some(true) {
            Some("running".to_string())
        } else {
            Some("stopped".to_string())
        }
    }

    pub async fn send_stdin_command(&self, container_id: &str, command: &str) -> Result<()> {
        let attach = self
            .docker
            .attach_container(
                container_id,
                Some(AttachContainerOptions::<String> {
                    stdin: Some(true),
                    stdout: Some(false),
                    stderr: Some(false),
                    stream: Some(true),
                    logs: Some(false),
                    detach_keys: None,
                }),
            )
            .await
            .context("attach container stdin")?;
        let mut input = attach.input;
        let line = format!("{}\n", command.trim_end_matches(&['\r', '\n'][..]));
        input
            .write_all(line.as_bytes())
            .await
            .context("write command to container stdin")?;
        input.flush().await.ok();
        Ok(())
    }

    pub async fn logs_tail(&self, container_id: &str, tail: &str) -> Result<Vec<String>> {
        let mut stream = self.docker.logs(
            container_id,
            Some(LogsOptions::<String> {
                stdout: true,
                stderr: true,
                tail: tail.to_string(),
                ..Default::default()
            }),
        );
        let mut lines = Vec::new();
        while let Some(item) = stream.next().await {
            if let Ok(output) = item {
                let s = String::from_utf8_lossy(&output.into_bytes()).to_string();
                for line in s.lines() {
                    lines.push(line.to_string());
                }
            }
        }
        Ok(lines)
    }

    pub fn logs_stream(
        &self,
        container_id: &str,
        tail: &str,
    ) -> impl futures_util::Stream<Item = Result<String, bollard::errors::Error>> + '_ {
        self.docker
            .logs(
                container_id,
                Some(LogsOptions::<String> {
                    follow: true,
                    stdout: true,
                    stderr: true,
                    tail: tail.to_string(),
                    ..Default::default()
                }),
            )
            .map(|res| res.map(|out| String::from_utf8_lossy(&out.into_bytes()).to_string()))
    }

    pub async fn stats_once(&self, container_id: &str) -> Result<ContainerStats> {
        let mut stream = self.docker.stats(
            container_id,
            Some(StatsOptions {
                stream: false,
                one_shot: true,
            }),
        );
        let Some(Ok(stat)) = stream.next().await else {
            return Err(anyhow::anyhow!("no stats returned"));
        };

        let cpu_delta = stat
            .cpu_stats
            .cpu_usage
            .total_usage
            .saturating_sub(stat.precpu_stats.cpu_usage.total_usage) as f64;
        let sys_delta =
            stat.cpu_stats
                .system_cpu_usage
                .unwrap_or(0)
                .saturating_sub(stat.precpu_stats.system_cpu_usage.unwrap_or(0)) as f64;
        let online_cpus = stat.cpu_stats.online_cpus.unwrap_or(1) as f64;
        let cpu_percent = if sys_delta > 0.0 && cpu_delta > 0.0 {
            (cpu_delta / sys_delta) * online_cpus * 100.0
        } else {
            0.0
        };

        let mem_usage = stat.memory_stats.usage.unwrap_or(0) as f64;
        let mem_limit = stat.memory_stats.limit.unwrap_or(1) as f64;
        let mem_percent = if mem_limit > 0.0 {
            (mem_usage / mem_limit) * 100.0
        } else {
            0.0
        };

        let mut rx = 0u64;
        let mut tx = 0u64;
        if let Some(nets) = stat.networks {
            for (_, net) in nets {
                rx += net.rx_bytes;
                tx += net.tx_bytes;
            }
        }

        Ok(ContainerStats {
            cpu_percent,
            memory_used_mb: mem_usage / (1024.0 * 1024.0),
            memory_limit_mb: mem_limit / (1024.0 * 1024.0),
            memory_percent: mem_percent,
            network_rx_bytes: rx,
            network_tx_bytes: tx,
        })
    }
}
