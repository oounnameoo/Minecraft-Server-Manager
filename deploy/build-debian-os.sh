#!/usr/bin/env bash
# Build a bootable Debian 12 (Bookworm) Linux OS ISO with the Debian Linux kernel
# and Minecraft Server Manager (MCM) pre-installed and auto-starting on boot.
#
# Usage (on Debian/Ubuntu, or via Docker on macOS/Linux):
#   sudo ./deploy/build-debian-os.sh [--arch amd64|arm64]
#
# Output:
#   dist/mcm-os-debian-12-<arch>.iso
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
ARCH="$(dpkg --print-architecture 2>/dev/null || uname -m)"
case "$ARCH" in
  x86_64) ARCH="amd64" ;;
  aarch64) ARCH="arm64" ;;
esac

while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) ARCH="$2"; shift 2 ;;
    *) echo "Unknown option: $1" >&2; exit 1 ;;
  esac
done

if [[ "$(id -u)" -ne 0 ]] || ! command -v lb >/dev/null 2>&1; then
  if command -v docker >/dev/null 2>&1; then
    echo "[mcm-os] Running Debian live-build inside privileged debian:bookworm container (${ARCH})..."
    exec docker run --rm --privileged \
      -v "$REPO_DIR:/src" \
      -w /src \
      --platform "linux/${ARCH}" \
      debian:bookworm \
      bash -c "
        apt-get update -qq &&
        apt-get install -y -qq live-build curl build-essential pkg-config cmake dpkg-dev ca-certificates &&
        if ! command -v cargo >/dev/null 2>&1; then
          curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y &&
          export PATH=\"\$HOME/.cargo/bin:\$PATH\";
        fi &&
        ./deploy/build-debian-os.sh --arch ${ARCH}
      "
  fi
  echo "Error: run as root on Debian/Ubuntu with 'live-build' installed, or with Docker available." >&2
  exit 1
fi

echo "[mcm-os] Building MCM .deb package for ${ARCH}..."
"$SCRIPT_DIR/build-deb.sh" --arch "$ARCH"
DEB_FILE="$(ls -1 "$REPO_DIR/dist/mcm_"*"_${ARCH}.deb" | head -n1)"

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT
cd "$WORK_DIR"

echo "[mcm-os] Configuring Debian 12 (Bookworm) live-build with Linux kernel + MCM..."
lb config \
  --distribution bookworm \
  --architecture "$ARCH" \
  --binary-images iso-hybrid \
  --archive-areas "main contrib non-free-firmware" \
  --bootappend-live "boot=live components hostname=mcm-server quiet" \
  --debian-installer live

mkdir -p config/package-lists config/packages.chroot config/hooks/live

cat > config/package-lists/mcm.list.chroot <<'EOF'
systemd-sysv
network-manager
openssh-server
ufw
curl
ca-certificates
openssl
openjdk-21-jre-headless
sudo
EOF

cp "$DEB_FILE" config/packages.chroot/

cat > config/hooks/live/99-mcm-appliance.hook.chroot <<'EOF'
#!/bin/sh
set -e
systemctl enable mcm.service || true
systemctl enable ssh || true
if command -v ufw >/dev/null 2>&1; then
  ufw allow 22/tcp || true
  ufw allow 8080/tcp || true
  ufw allow 25565:25665/tcp || true
  ufw allow 25565:25665/udp || true
fi
EOF
chmod 755 config/hooks/live/99-mcm-appliance.hook.chroot

echo "[mcm-os] Building Debian Linux kernel + rootfs + ISO (this takes a few minutes)..."
lb build

ISO_OUT="$(ls -1 "$WORK_DIR"/*.iso | head -n1)"
mkdir -p "$REPO_DIR/dist"
TARGET_ISO="$REPO_DIR/dist/mcm-os-debian-12-${ARCH}.iso"
mv -f "$ISO_OUT" "$TARGET_ISO"
echo "[mcm-os] Built bootable Debian Linux + MCM ISO: $TARGET_ISO"
ls -lh "$TARGET_ISO"
