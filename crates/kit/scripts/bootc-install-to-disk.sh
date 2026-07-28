#!/bin/bash
set -euo pipefail
@@RUST_LOG@@
LOOP=$(@@SUDO@@losetup -fP --show @@DISK_PATH@@)
echo "Loop device: $LOOP"
trap '@@SUDO@@losetup -d $LOOP 2>/dev/null' EXIT

printf '%s' '@@SSH_PUBKEY_B64@@' | base64 -d > /dev/shm/bcvk-ssh-key.pub

echo "Running bootc install to-disk..."
podman run --rm --privileged --pid=host --net=none \
  -v /dev:/dev \
  -v /dev/shm:/dev/shm \
  -v /var/lib/containers:/var/lib/containers \
@@LABEL_LINE@@  @@IMAGE@@ bootc install to-disk \
  --generic-image --skip-fetch-check --wipe \
  --root-ssh-authorized-keys /dev/shm/bcvk-ssh-key.pub \
  @@BOOTC_ARGS@@ $LOOP

rm -f /dev/shm/bcvk-ssh-key.pub

echo "Installation complete!"
