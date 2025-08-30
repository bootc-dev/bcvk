#!/bin/bash
set -euo pipefail

# Check if we're in debug mode
DEBUG_MODE="${DEBUG_MODE:-false}"

vmlinuz=$(ls /usr/lib/modules/*/vmlinuz)
kdir=$(dirname $vmlinuz)
initramfs=$(echo $kdir/initramfs.img)

# Create the hybrid rootfs directory
mkdir /run/tmproot
cd /run/tmproot

# Bind mount host /usr to our hybrid root
mkdir usr
mount --bind /run/hostusr usr
# Create essential symlinks that typically point to /usr
ln -sf usr/bin bin
ln -sf usr/lib lib
ln -sf usr/lib64 lib64
ln -sf usr/sbin sbin
mkdir -p {etc,var,dev,proc,run,sys,tmp}

# This directory is shared between the outer container (podman) and the inner (bwrap)
mkdir /run/inner-shared

# Create other essential directories from the container's existing structure

BWRAP_ARGS=(
    --bind /run/tmproot /
    --proc /proc
    --dev-bind /dev /dev
    --tmpfs /run
    --tmpfs /tmp
    --bind /run/inner-shared /run/inner-shared
    )

# Verify KVM access
bwrap "${BWRAP_ARGS[@]}" test -w /dev/kvm

# Start virtiofsd in the background using bwrap
bwrap "${BWRAP_ARGS[@]}" \
    --ro-bind /run/source-image /run/source-image \
    -- \
    /usr/libexec/virtiofsd \
    --socket-path /run/inner-shared/virtiofs.sock \
    --shared-dir /run/source-image \
    --cache always \
    --sandbox none &

# Wait for virtiofsd to create the socket
sleep 2

# Build QEMU command line arguments
mkdir /run/qemu
touch /run/qemu/{kernel,initramfs}
mount --bind -o ro $vmlinuz /run/qemu/kernel
mount --bind -o ro $initramfs /run/qemu/initramfs
QEMU_ARGS=(
    -m {{MEMORY}}M
    -smp {{VCPUS}}
    -enable-kvm
    -cpu host
    -kernel /run/qemu/kernel
    -initrd /run/qemu/initramfs
    -chardev socket,id=char0,path=/run/inner-shared/virtiofs.sock
    -device vhost-user-fs-pci,queue-size=1024,chardev=char0,tag=rootfs
    -object memory-backend-memfd,id=mem,share=on,size={{MEMORY}}M
    -numa node,memdev=mem
    -append "rootfstype=virtiofs root=rootfs selinux=0 systemd.volatile=overlay systemd.set_credential=passwd.plaintext-password.root:12345 {{EXTRA_ARGS}}"
)

{{CONSOLE_QEMU_ARGS}}

# Execute QEMU or debug shell based on mode
if [ "$DEBUG_MODE" = "true" ]; then
    echo "=== DEBUG MODE: Dropping into bash shell ==="
    exec bash
else
    # Execute QEMU normally
    exec bwrap "${BWRAP_ARGS[@]}" --ro-bind /run/qemu /run/qemu -- qemu-kvm "${QEMU_ARGS[@]}"
fi