#!/usr/bin/env bash
# Provision a Debian/Ubuntu Linux server as a Home Assistant-style MCM Appliance:
#   - Installs MCM + mcm-cli as a systemd service (via deploy/install-linux.sh)
#   - Configures mDNS (avahi-daemon) so the panel is reachable at https://mcm.local:8080
#   - Configures UFW firewall for SSH, panel port, and Minecraft server ports
#   - Adds a boot/login appliance status banner (TTY1 + SSH MOTD)
#
# Usage: sudo ./deploy/mcm-appliance.sh [--hostname mcm] [--port 8080] [--data-dir /var/lib/mcm] [--http-only] [--non-interactive]
set -Eeuo pipefail

HOSTNAME_VAL="mcm"
PORT="8080"
DATA_DIR="/var/lib/mcm"
HTTP_ONLY="false"
NON_INTERACTIVE="false"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

say() { printf '[mcm-appliance] %s\n' "$*"; }
die() { printf '[mcm-appliance] ERROR: %s\n' "$*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --hostname)
      [[ $# -ge 2 ]] || die "--hostname requires a value"
      HOSTNAME_VAL="$2"
      shift 2
      ;;
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
    -h|--help)
      cat <<'EOF'
Usage: sudo ./deploy/mcm-appliance.sh [OPTIONS]

Options:
  --hostname <name>     mDNS hostname (default: mcm -> https://mcm.local:8080)
  --port <port>         Panel port (default: 8080)
  --data-dir <path>     Data directory (default: /var/lib/mcm)
  --http-only           Serve HTTP instead of self-signed HTTPS
  --non-interactive     Do not prompt
EOF
      exit 0
      ;;
    *)
      die "Unknown option: $1"
      ;;
  esac
done

[[ "$(id -u)" -eq 0 ]] || die "Please run as root or with sudo."

# 1. Install core MCM service + mcm-cli
INSTALL_ARGS=(--port "$PORT" --data-dir "$DATA_DIR")
[[ "$HTTP_ONLY" == "true" ]] && INSTALL_ARGS+=(--http-only)
[[ "$NON_INTERACTIVE" == "true" ]] && INSTALL_ARGS+=(--non-interactive)
bash "$SCRIPT_DIR/install-linux.sh" "${INSTALL_ARGS[@]}"

# 2. Install appliance packages (mDNS + firewall)
say "Installing appliance packages (avahi-daemon, ufw)..."
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq avahi-daemon libnss-mdns ufw >/dev/null

# 3. Configure hostname & mDNS (mcm.local)
if command -v hostnamectl >/dev/null 2>&1 && [[ -d /run/systemd/system ]]; then
  hostnamectl set-hostname "$HOSTNAME_VAL" || true
else
  printf '%s\n' "$HOSTNAME_VAL" > /etc/hostname
fi

if [[ -d /run/systemd/system ]]; then
  systemctl enable --now avahi-daemon >/dev/null 2>&1 || true
fi

# 4. Configure UFW firewall (if available and not inside a container without netfilter)
if command -v ufw >/dev/null 2>&1; then
  ufw allow 22/tcp comment 'SSH' >/dev/null 2>&1 || true
  ufw allow 5353/udp comment 'mDNS (mcm.local)' >/dev/null 2>&1 || true
  ufw allow "${PORT}/tcp" comment 'MCM Web Panel' >/dev/null 2>&1 || true
  ufw allow 25565:25665/tcp comment 'Minecraft TCP' >/dev/null 2>&1 || true
  ufw allow 25565:25665/udp comment 'Minecraft UDP' >/dev/null 2>&1 || true
  ufw --force enable >/dev/null 2>&1 || true
fi

# 5. Install Home Assistant-style welcome banner for SSH & TTY1
PROTO="https"
[[ "$HTTP_ONLY" == "true" ]] && PROTO="http"

cat > /etc/profile.d/mcm-appliance.sh <<EOF
# MCM Appliance welcome banner
if [ -t 1 ]; then
  _mcm_ip="\$(hostname -I 2>/dev/null | awk '{print \$1}')"
  [ -z "\$_mcm_ip" ] && _mcm_ip="<server-ip>"
  echo ""
  echo "  __  __  ___ __  __    ___  ___ "
  echo " |  \\/  |/ __|  \\/  |  / _ \\/ __|"
  echo " | |\\/| | (__| |\\/| | | (_) \\__ \\"
  echo " |_|  |_|\\___|_|  |_|  \\___/|___/"
  echo ""
  echo "  Panel URL:   ${PROTO}://${HOSTNAME_VAL}.local:${PORT}"
  echo "               ${PROTO}://\${_mcm_ip}:${PORT}"
  echo "  CLI:         mcm-cli --help   (Proxmox-style: mcm-cli list, status, start, console)"
  echo "  Service:     systemctl status mcm"
  echo ""
  unset _mcm_ip
fi
EOF
chmod 644 /etc/profile.d/mcm-appliance.sh

cat > /etc/issue <<EOF
MCM Linux Appliance (\n \l)

  Panel URL:  ${PROTO}://${HOSTNAME_VAL}.local:${PORT}
  CLI:        mcm-cli status | mcm-cli list

EOF

say "MCM Appliance setup complete!"
say "Access the panel at ${PROTO}://${HOSTNAME_VAL}.local:${PORT} (or ${PROTO}://<SERVER_IP>:${PORT})"
