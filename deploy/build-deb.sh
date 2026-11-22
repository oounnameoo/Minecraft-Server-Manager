#!/usr/bin/env bash
# Build a Debian/Ubuntu .deb package for MCM so users can install via:
#   sudo apt install ./dist/mcm_<version>_<arch>.deb
# Requires dpkg-deb and cargo (run on Linux, or pass prebuilt --bin-dir).
set -Eeuo pipefail

VERSION="0.1.0"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
ARCH="$(dpkg --print-architecture 2>/dev/null || uname -m)"
case "$ARCH" in
  x86_64) ARCH="amd64" ;;
  aarch64) ARCH="arm64" ;;
esac
BIN_DIR=""
TARGET=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) ARCH="$2"; shift 2 ;;
    --bin-dir) BIN_DIR="$2"; shift 2 ;;
    --target) TARGET="$2"; shift 2 ;;
    *) echo "Unknown option: $1" >&2; exit 1 ;;
  esac
done

command -v dpkg-deb >/dev/null 2>&1 || {
  echo "dpkg-deb is required (install dpkg on Debian/Ubuntu or brew install dpkg on macOS)" >&2
  exit 1
}

if [[ -z "$BIN_DIR" ]]; then
  if [[ -n "$TARGET" ]]; then
    (cd "$REPO_DIR/rust" && cargo build --release --workspace --target "$TARGET")
    BIN_DIR="$REPO_DIR/rust/target/$TARGET/release"
  else
    (cd "$REPO_DIR/rust" && cargo build --release --workspace)
    BIN_DIR="$REPO_DIR/rust/target/release"
  fi
fi

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
PKG_ROOT="$STAGE/mcm_${VERSION}_${ARCH}"

mkdir -p "$PKG_ROOT/DEBIAN" \
  "$PKG_ROOT/usr/bin" \
  "$PKG_ROOT/lib/systemd/system" \
  "$PKG_ROOT/etc/mcm"

install -m 755 "$BIN_DIR/mcm" "$PKG_ROOT/usr/bin/mcm"
install -m 755 "$BIN_DIR/mcm-cli" "$PKG_ROOT/usr/bin/mcm-cli"

cat > "$PKG_ROOT/lib/systemd/system/mcm.service" <<'EOF'
[Unit]
Description=MCM - Minecraft Server Manager
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=mcm
Group=mcm
EnvironmentFile=/etc/mcm/mcm.env
WorkingDirectory=/var/lib/mcm
ExecStart=/usr/bin/mcm
Restart=always
RestartSec=3
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF

cat > "$PKG_ROOT/etc/mcm/mcm.env" <<'EOF'
# MCM System Service Configuration
MCM_ADDR=:8080
MCM_DATA_DIR=/var/lib/mcm
MCM_BACKUP_DIR=/var/lib/mcm/backups
MCM_DB_PATH=/var/lib/mcm/mcm.db
MCM_PORT_RANGE=25565-25665
MCM_HTTP_ONLY=false
MCM_SESSION_SECRET=
EOF

cat > "$PKG_ROOT/DEBIAN/conffiles" <<'EOF'
/etc/mcm/mcm.env
EOF

cat > "$PKG_ROOT/DEBIAN/control" <<EOF
Package: mcm
Version: ${VERSION}
Section: games
Priority: optional
Architecture: ${ARCH}
Depends: adduser, openssl, ca-certificates
Recommends: openjdk-21-jre-headless | openjdk-17-jre-headless | temurin-21-jre | default-jre-headless
Maintainer: MCM <noreply@example.com>
Description: Minecraft Server Manager (native Rust panel and CLI)
 Native Linux Minecraft server management panel and Proxmox-style CLI (mcm-cli).
EOF

cat > "$PKG_ROOT/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e

if ! getent group mcm >/dev/null 2>&1; then
  addgroup --system mcm
fi
if ! id -u mcm >/dev/null 2>&1; then
  adduser --system --ingroup mcm --home /var/lib/mcm --no-create-home --shell /usr/sbin/nologin mcm
fi

mkdir -p /var/lib/mcm /var/lib/mcm/backups /var/lib/mcm/servers
chown -R mcm:mcm /var/lib/mcm

if grep -q '^MCM_SESSION_SECRET=$' /etc/mcm/mcm.env; then
  SECRET="$(openssl rand -hex 32)"
  sed -i "s|^MCM_SESSION_SECRET=$|MCM_SESSION_SECRET=${SECRET}|" /etc/mcm/mcm.env
fi
chown root:mcm /etc/mcm/mcm.env
chmod 640 /etc/mcm/mcm.env

if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload || true
  systemctl enable --now mcm.service || true
fi
EOF
chmod 755 "$PKG_ROOT/DEBIAN/postinst"

cat > "$PKG_ROOT/DEBIAN/prerm" <<'EOF'
#!/bin/sh
set -e
if [ "$1" = "remove" ] && command -v systemctl >/dev/null 2>&1; then
  systemctl stop mcm.service || true
  systemctl disable mcm.service || true
fi
EOF
chmod 755 "$PKG_ROOT/DEBIAN/prerm"

mkdir -p "$REPO_DIR/dist"
OUT_DEB="$REPO_DIR/dist/mcm_${VERSION}_${ARCH}.deb"
dpkg-deb --build "$PKG_ROOT" "$OUT_DEB"
echo "Built Debian package: $OUT_DEB"
echo "Install on Debian/Ubuntu with: sudo apt install $OUT_DEB"
