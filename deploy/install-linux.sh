#!/usr/bin/env bash
# System-wide installer for MCM (Minecraft Container Manager) on Debian / Ubuntu servers.
# Usage: sudo ./deploy/install-linux.sh [--port 8080] [--data-dir /var/lib/mcm] [--non-interactive] [--uninstall]
set -Eeuo pipefail

APP_NAME="mcm"
SERVICE_NAME="mcm.service"
BIN_PATH="/usr/local/bin/mcm"
CLI_BIN_PATH="/usr/local/bin/mcm-cli"
CONFIG_DIR="/etc/mcm"
ENV_FILE="$CONFIG_DIR/mcm.env"
UNIT_PATH="/etc/systemd/system/$SERVICE_NAME"
DEFAULT_DATA_DIR="/var/lib/mcm"
DEFAULT_PORT="8080"

DATA_DIR="$DEFAULT_DATA_DIR"
PORT="$DEFAULT_PORT"
HTTP_ONLY="false"
NON_INTERACTIVE="false"
UNINSTALL="false"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

say() { printf '[mcm-install] %s\n' "$*"; }
die() { printf '[mcm-install] ERROR: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: sudo ./deploy/install-linux.sh [OPTIONS]

Options:
  --port <port>         Panel HTTP/HTTPS port (default: 8080)
  --data-dir <path>     Persistent data directory (default: /var/lib/mcm)
  --http-only           Serve HTTP instead of self-signed HTTPS
  --non-interactive     Do not prompt; use flags or defaults
  --uninstall           Stop/disable service and remove binary & unit (keeps /var/lib/mcm and /etc/mcm)
  -h, --help            Show this help message
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --port)
      [[ $# -ge 2 ]] || die "--port requires a value"
      PORT="$2"
      shift 2
      ;;
    --data-dir)
      [[ $# -ge 2 ]] || die "--data-dir requires a value"
      DATA_DIR="$2"
      shift 2
      ;;
    --http-only)
      HTTP_ONLY="true"
      shift
      ;;
    --non-interactive)
      NON_INTERACTIVE="true"
      shift
      ;;
    --uninstall)
      UNINSTALL="true"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "Unknown option: $1"
      ;;
  esac
done

[[ "$(id -u)" -eq 0 ]] || die "Please run as root or with sudo."

if [[ "$UNINSTALL" == "true" ]]; then
  say "Uninstalling $APP_NAME system service..."
  if command -v systemctl >/dev/null 2>&1; then
    systemctl stop "$SERVICE_NAME" 2>/dev/null || true
    systemctl disable "$SERVICE_NAME" 2>/dev/null || true
  fi
  rm -f "$UNIT_PATH" "$BIN_PATH" "$CLI_BIN_PATH"
  if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload 2>/dev/null || true
  fi
  say "Removed $BIN_PATH and $UNIT_PATH. Data in $DATA_DIR and config in $CONFIG_DIR were preserved."
  exit 0
fi

# Validate OS (Debian / Ubuntu)
if [[ -r /etc/os-release ]]; then
  # shellcheck disable=SC1091
  . /etc/os-release
  case "${ID:-}" in
    debian|ubuntu|raspbian|linuxmint|pop)
      say "Detected supported OS: ${PRETTY_NAME:-$ID}"
      ;;
    *)
      say "Warning: '${ID:-unknown}' is not explicitly Debian/Ubuntu. Proceeding with apt-based setup..."
      ;;
  esac
else
  die "Cannot detect OS (/etc/os-release missing)."
fi

[[ "$PORT" =~ ^[0-9]+$ ]] && (( PORT >= 1 && PORT <= 65535 )) || die "Invalid port: $PORT"
[[ "$DATA_DIR" == /* ]] || die "--data-dir must be an absolute path: $DATA_DIR"

if [[ "$NON_INTERACTIVE" != "true" && -t 0 ]]; then
  read -r -p "Panel port [$PORT]: " input_port || true
  if [[ -n "${input_port:-}" ]]; then PORT="$input_port"; fi
  read -r -p "Data directory [$DATA_DIR]: " input_dir || true
  if [[ -n "${input_dir:-}" ]]; then DATA_DIR="$input_dir"; fi
fi

say "Installing base dependencies via apt..."
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq ca-certificates curl git openssl

if ! command -v docker >/dev/null 2>&1; then
  say "Docker not found. Installing Docker Engine (docker.io)..."
  apt-get install -y -qq docker.io
fi

if command -v systemctl >/dev/null 2>&1; then
  systemctl enable --now docker >/dev/null 2>&1 || true
fi

docker info >/dev/null 2>&1 || die "Docker daemon is not running or inaccessible."

if [[ -x "$SCRIPT_DIR/bin/mcm" && -x "$SCRIPT_DIR/bin/mcm-cli" ]]; then
  say "Installing pre-built $APP_NAME and mcm-cli binaries from $SCRIPT_DIR/bin..."
  install -m 755 "$SCRIPT_DIR/bin/mcm" "$BIN_PATH"
  install -m 755 "$SCRIPT_DIR/bin/mcm-cli" "$CLI_BIN_PATH"
  say "Installed binaries to $BIN_PATH and $CLI_BIN_PATH"
else
  say "Building $APP_NAME binary using Docker multi-stage build..."
  BUILD_COMMIT="$(git -C "$REPO_DIR" rev-parse HEAD 2>/dev/null || echo unknown)"
  BUILD_DIRTY="false"
  if [[ -n "$(git -C "$REPO_DIR" status --porcelain 2>/dev/null || true)" ]]; then
    BUILD_DIRTY="true"
  fi

  IMAGE_TAG="mcm-linux-builder:latest"
  docker build \
    --target runtime \
    --build-arg "MCM_BUILD_COMMIT=$BUILD_COMMIT" \
    --build-arg "MCM_BUILD_DIRTY=$BUILD_DIRTY" \
    -t "$IMAGE_TAG" \
    "$REPO_DIR"

  TMP_CONTAINER="$(docker create "$IMAGE_TAG")"
  trap 'docker rm -f "$TMP_CONTAINER" >/dev/null 2>&1 || true' EXIT
  docker cp "$TMP_CONTAINER:/usr/local/bin/mcm" "$BIN_PATH.tmp"
  chmod 755 "$BIN_PATH.tmp"
  mv -f "$BIN_PATH.tmp" "$BIN_PATH"
  docker cp "$TMP_CONTAINER:/usr/local/bin/mcm-cli" "$CLI_BIN_PATH.tmp"
  chmod 755 "$CLI_BIN_PATH.tmp"
  mv -f "$CLI_BIN_PATH.tmp" "$CLI_BIN_PATH"
  docker rm -f "$TMP_CONTAINER" >/dev/null 2>&1 || true
  trap - EXIT
  say "Installed binaries to $BIN_PATH and $CLI_BIN_PATH"
fi

# Create mcm system user and add to docker group
if ! getent group "$APP_NAME" >/dev/null 2>&1; then
  groupadd --system "$APP_NAME"
fi
if ! id -u "$APP_NAME" >/dev/null 2>&1; then
  useradd --system --gid "$APP_NAME" --home-dir "$DATA_DIR" --shell /usr/sbin/nologin "$APP_NAME"
fi
if getent group docker >/dev/null 2>&1; then
  usermod -aG docker "$APP_NAME"
fi

# Prepare directories
mkdir -p "$DATA_DIR" "$DATA_DIR/backups" "$DATA_DIR/servers" "$CONFIG_DIR"
chown -R "$APP_NAME:$APP_NAME" "$DATA_DIR"

# Generate or preserve configuration
if [[ ! -f "$ENV_FILE" ]]; then
  SESSION_SECRET="$(openssl rand -hex 32)"
  cat > "$ENV_FILE" <<EOF
# MCM System Service Configuration
MCM_ADDR=:$PORT
MCM_DATA_DIR=$DATA_DIR
MCM_DATA_DIR_HOST=$DATA_DIR
MCM_BACKUP_DIR=$DATA_DIR/backups
MCM_DB_PATH=$DATA_DIR/mcm.db
MCM_CONTAINER_RUNTIME=docker
DOCKER_HOST=unix:///var/run/docker.sock
MCM_PORT_RANGE=25565-25665
MCM_SERVER_IMAGE=eclipse-temurin:21-jre-alpine
MCM_HTTP_ONLY=$HTTP_ONLY
MCM_SESSION_SECRET=$SESSION_SECRET
EOF
  say "Created configuration at $ENV_FILE"
else
  say "Preserving existing configuration at $ENV_FILE"
fi
chown root:"$APP_NAME" "$ENV_FILE"
chmod 640 "$ENV_FILE"

# Install systemd service unit
cat > "$UNIT_PATH" <<EOF
[Unit]
Description=MCM - Minecraft Container Manager
After=network-online.target docker.service
Wants=network-online.target
Requires=docker.service

[Service]
Type=simple
User=$APP_NAME
Group=$APP_NAME
SupplementaryGroups=docker
ExecStart=$BIN_PATH
EnvironmentFile=$ENV_FILE
WorkingDirectory=$DATA_DIR
Restart=always
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF

if command -v systemctl >/dev/null 2>&1 && [[ -d /run/systemd/system ]]; then
  systemctl daemon-reload
  systemctl enable --now "$SERVICE_NAME"
  systemctl restart "$SERVICE_NAME"
  say "Systemd service $SERVICE_NAME is active."
else
  say "systemd is not running as PID 1; wrote $UNIT_PATH."
fi

PROTO="https"
[[ "$HTTP_ONLY" == "true" ]] && PROTO="http"
say "MCM installation complete! Access the panel at $PROTO://<SERVER_IP>:$PORT"
