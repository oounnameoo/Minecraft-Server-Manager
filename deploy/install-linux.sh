#!/usr/bin/env bash
# Native Linux installer for MCM (Minecraft Server Manager) on Debian / Ubuntu.
# No Docker or Podman required — Minecraft servers run natively via Java + systemd.
# Usage: sudo ./deploy/install-linux.sh [--port 8080] [--data-dir /var/lib/mcm] [--http-only] [--non-interactive] [--uninstall]
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
  --uninstall           Stop/disable service and remove binaries & unit (keeps /var/lib/mcm and /etc/mcm)
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

[[ "$PORT" =~ ^[0-9]+$ ]] && (( PORT >= 1 && PORT <= 65535 )) || die "Invalid port: $PORT"
[[ "$DATA_DIR" == /* ]] || die "--data-dir must be an absolute path: $DATA_DIR"

if [[ "$NON_INTERACTIVE" != "true" && -t 0 ]]; then
  read -r -p "Panel port [$PORT]: " input_port || true
  if [[ -n "${input_port:-}" ]]; then PORT="$input_port"; fi
  read -r -p "Data directory [$DATA_DIR]: " input_dir || true
  if [[ -n "${input_dir:-}" ]]; then DATA_DIR="$input_dir"; fi
fi

say "Installing Java runtime and base dependencies via apt..."
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
JAVA_PKG="default-jre-headless"
for candidate in openjdk-21-jre-headless openjdk-17-jre-headless default-jre-headless; do
  if apt-cache show "$candidate" >/dev/null 2>&1; then
    JAVA_PKG="$candidate"
    break
  fi
done
apt-get install -y -qq ca-certificates curl openssl "$JAVA_PKG"

if [[ -x "$SCRIPT_DIR/bin/mcm" && -x "$SCRIPT_DIR/bin/mcm-cli" ]]; then
  say "Installing pre-built $APP_NAME and mcm-cli binaries..."
  install -m 755 "$SCRIPT_DIR/bin/mcm" "$BIN_PATH"
  install -m 755 "$SCRIPT_DIR/bin/mcm-cli" "$CLI_BIN_PATH"
else
  if ! command -v cargo >/dev/null 2>&1; then
    say "Rust toolchain not found; installing build dependencies and rustup..."
    apt-get install -y -qq build-essential pkg-config cmake
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    export PATH="$HOME/.cargo/bin:$PATH"
  fi
  say "Building $APP_NAME and mcm-cli from source..."
  (cd "$REPO_DIR/rust" && cargo build --release --workspace)
  install -m 755 "$REPO_DIR/rust/target/release/mcm" "$BIN_PATH"
  install -m 755 "$REPO_DIR/rust/target/release/mcm-cli" "$CLI_BIN_PATH"
fi

if ! getent group "$APP_NAME" >/dev/null 2>&1; then
  groupadd --system "$APP_NAME"
fi
if ! id -u "$APP_NAME" >/dev/null 2>&1; then
  useradd --system --gid "$APP_NAME" --home-dir "$DATA_DIR" --shell /usr/sbin/nologin "$APP_NAME"
fi

mkdir -p "$DATA_DIR" "$DATA_DIR/backups" "$DATA_DIR/servers" "$CONFIG_DIR"
chown -R "$APP_NAME:$APP_NAME" "$DATA_DIR"

if [[ ! -f "$ENV_FILE" ]]; then
  SESSION_SECRET="$(openssl rand -hex 32)"
  cat > "$ENV_FILE" <<EOF
# MCM System Service Configuration
MCM_ADDR=:$PORT
MCM_DATA_DIR=$DATA_DIR
MCM_BACKUP_DIR=$DATA_DIR/backups
MCM_DB_PATH=$DATA_DIR/mcm.db
MCM_PORT_RANGE=25565-25665
MCM_HTTP_ONLY=$HTTP_ONLY
MCM_SESSION_SECRET=$SESSION_SECRET
EOF
  say "Created configuration at $ENV_FILE"
else
  say "Preserving existing configuration at $ENV_FILE"
fi
chown root:"$APP_NAME" "$ENV_FILE"
chmod 640 "$ENV_FILE"

cat > "$UNIT_PATH" <<EOF
[Unit]
Description=MCM - Minecraft Server Manager
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$APP_NAME
Group=$APP_NAME
EnvironmentFile=$ENV_FILE
WorkingDirectory=$DATA_DIR
ExecStart=$BIN_PATH
Restart=always
RestartSec=3
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF

if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload
  systemctl enable --now "$SERVICE_NAME"
  say "Service $SERVICE_NAME is enabled and running."
fi

PROTO="https"
if [[ "$HTTP_ONLY" == "true" ]]; then
  PROTO="http"
fi
HOST_IP="$(hostname -I 2>/dev/null | awk '{print $1}' || echo 'localhost')"

say "Installation complete!"
say "  Panel URL : ${PROTO}://${HOST_IP}:${PORT}"
say "  CLI       : mcm-cli --url ${PROTO}://localhost:${PORT} -k login"
say "  Logs      : journalctl -u $SERVICE_NAME -f"
