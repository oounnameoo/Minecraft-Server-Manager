#!/usr/bin/env bash
# Package MCM Rust Linux binaries + installers into a portable tarball.
# Usage: ./deploy/package-linux.sh [--arch amd64|arm64] [--rebuild]
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
ARCH="$(docker info --format '{{.Architecture}}' 2>/dev/null || uname -m)"
case "$ARCH" in
  x86_64|amd64) ARCH="amd64" ;;
  aarch64|arm64) ARCH="arm64" ;;
esac
REBUILD="false"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch)
      ARCH="$2"
      shift 2
      ;;
    --rebuild)
      REBUILD="true"
      shift
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

IMAGE_TAG="minecraft-container-manager-mcm:latest"
if [[ "$REBUILD" == "true" ]] || ! docker image inspect "$IMAGE_TAG" >/dev/null 2>&1; then
  docker build --platform "linux/${ARCH}" --target runtime -t "$IMAGE_TAG" "$REPO_DIR"
fi

STAGE_DIR="$(mktemp -d)"
trap 'rm -rf "$STAGE_DIR"' EXIT
BUNDLE_DIR="$STAGE_DIR/mcm-linux-${ARCH}"
mkdir -p "$BUNDLE_DIR/bin"

CID="$(docker create --platform "linux/${ARCH}" "$IMAGE_TAG")"
trap 'docker rm -f "$CID" >/dev/null 2>&1 || true; rm -rf "$STAGE_DIR"' EXIT
docker cp "$CID:/usr/local/bin/mcm" "$BUNDLE_DIR/bin/mcm"
docker cp "$CID:/usr/local/bin/mcm-cli" "$BUNDLE_DIR/bin/mcm-cli"
chmod 755 "$BUNDLE_DIR/bin/mcm" "$BUNDLE_DIR/bin/mcm-cli"
docker rm -f "$CID" >/dev/null 2>&1 || true

cp "$SCRIPT_DIR/install-linux.sh" "$BUNDLE_DIR/install-linux.sh"
cp "$SCRIPT_DIR/mcm-appliance.sh" "$BUNDLE_DIR/mcm-appliance.sh"
cp "$SCRIPT_DIR/cloud-init.yaml" "$BUNDLE_DIR/cloud-init.yaml"
cp "$REPO_DIR/mcmctl" "$BUNDLE_DIR/mcmctl"
chmod 755 "$BUNDLE_DIR/install-linux.sh" "$BUNDLE_DIR/mcm-appliance.sh" "$BUNDLE_DIR/mcmctl"

cat > "$BUNDLE_DIR/README-INSTALL.txt" <<'EOF'
MCM (Minecraft Container Manager) - Rust Linux Installation Bundle
==================================================================

1. Extract this archive on your Debian/Ubuntu Linux server:
   tar -xzf mcm-linux-*.tar.gz
   cd mcm-linux-*

2. Install MCM as a systemd service:
   sudo ./install-linux.sh --port 8080 --data-dir /var/lib/mcm

   (Or for full appliance setup with firewall/hostname/unattended upgrades:)
   sudo ./mcm-appliance.sh

3. Manage the service & CLI:
   sudo systemctl status mcm
   mcm-cli --url https://localhost:8080 -k login
   mcm-cli list
EOF

OUT_DIR="$REPO_DIR/dist"
mkdir -p "$OUT_DIR"
OUT_FILE="$OUT_DIR/mcm-linux-${ARCH}.tar.gz"
tar -C "$STAGE_DIR" -czf "$OUT_FILE" "mcm-linux-${ARCH}"
echo "Created installation bundle: $OUT_FILE"
ls -lh "$OUT_FILE"
