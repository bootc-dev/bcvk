#!/bin/bash
# bcvk anaconda setup script
#
# This script runs before anaconda to set up:
# - virtiofs mounts for host container storage and kickstart
# - container storage configuration for additionalImageStores
#
# Anaconda itself is run by the upstream anaconda-direct.service
set -euo pipefail

echo "bcvk: Setting up container storage for anaconda..."

# Mount host container storage via virtiofs
AIS=/run/virtiofs-mnt-hoststorage
if ! mountpoint -q "${AIS}" 2>/dev/null; then
    mkdir -p "${AIS}"
    mount -t virtiofs mount_hoststorage "${AIS}" || {
        echo "bcvk: ERROR: Failed to mount host container storage"
        exit 1
    }
fi

# Mount kickstart directory via virtiofs
KS_DIR=/run/virtiofs-mnt-kickstart
if ! mountpoint -q "${KS_DIR}" 2>/dev/null; then
    mkdir -p "${KS_DIR}"
    mount -t virtiofs mount_kickstart "${KS_DIR}" || {
        echo "bcvk: ERROR: Failed to mount kickstart directory"
        exit 1
    }
fi

# Configure containers to use host storage as additional image store
mkdir -p /etc/containers
cat > /etc/containers/storage.conf << 'EOF'
[storage]
driver = "overlay"
[storage.options]
additionalimagestores = ["/run/virtiofs-mnt-hoststorage"]
EOF

# Verify kickstart exists
if [ ! -f "${KS_DIR}/anaconda.ks" ]; then
    echo "bcvk: ERROR: Kickstart not found at ${KS_DIR}/anaconda.ks"
    exit 1
fi

# Copy kickstart to where anaconda expects it
# Anaconda looks for /run/install/ks.cfg when inst.ks is specified
mkdir -p /run/install
cp "${KS_DIR}/anaconda.ks" /run/install/ks.cfg
echo "bcvk: Installed kickstart to /run/install/ks.cfg"

echo "bcvk: Setup complete. Kickstart: ${KS_DIR}/anaconda.ks"
