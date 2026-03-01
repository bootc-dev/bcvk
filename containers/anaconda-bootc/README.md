# Anaconda Installer Container for bcvk

This container provides the anaconda installer for installing bootc container
images using kickstart files. It boots into `anaconda.target` and uses the
upstream anaconda systemd services.

## Overview

The container is based on `quay.io/fedora/fedora-bootc:42` with anaconda-tui
installed. It boots directly into `anaconda.target` and uses the upstream
`anaconda-direct.service` with a bcvk setup service that runs beforehand.

## Building

```bash
cd containers/anaconda-bootc
podman build -t localhost/anaconda-bootc:latest .
```

## How It Works

1. bcvk creates a target disk and generates a kickstart file
2. bcvk starts the VM with:
   - Host container storage mounted read-only via virtiofs
   - Kickstart file mounted via virtiofs at `/run/virtiofs-mnt-kickstart/`
   - Target disk attached via virtio-blk
   - Kernel args: `inst.notmux inst.ks=file:///run/virtiofs-mnt-kickstart/anaconda.ks`
3. The VM boots into `anaconda.target`
4. `bcvk-anaconda-setup.service` runs first to:
   - Mount virtiofs shares for container storage and kickstart
   - Configure `/etc/containers/storage.conf` with additionalImageStores
5. Upstream `anaconda-direct.service` runs anaconda (triggered by `inst.notmux`)
6. Kickstart `poweroff` directive powers off the VM after anaconda completes

## Integration with Upstream Anaconda

This container leverages upstream anaconda systemd infrastructure:

| Component | Source | Purpose |
|-----------|--------|---------|
| `anaconda.target` | Upstream | Default boot target for installation |
| `anaconda-direct.service` | Upstream | Runs anaconda without tmux |
| `bcvk-anaconda-setup.service` | bcvk | Sets up virtiofs mounts before anaconda (conditional on `bcvk.anaconda` kernel arg) |

## Kickstart Requirements

The user provides a kickstart with partitioning and locale settings.
bcvk injects:
- `ostreecontainer --transport=containers-storage --url=<image>`
- `%post` script to repoint bootc origin to the registry (unless `--no-repoint`)

**Important**: The target disk is available at `/dev/disk/by-id/virtio-output`.
bcvk also attaches a swap disk, so use `ignoredisk` to target the correct disk.

Example kickstart for BIOS boot:
```kickstart
text
lang en_US.UTF-8
keyboard us
timezone UTC --utc
network --bootproto=dhcp --activate

# Target only the output disk
ignoredisk --only-use=/dev/disk/by-id/virtio-output

zerombr
clearpart --all --initlabel

# Create required boot partitions (biosboot + /boot)
reqpart --add-boot
part / --fstype=xfs --grow

rootpw --lock
poweroff
```

## Installed Packages

See `packages.txt` for the full list. Key packages:
- **anaconda-tui**: Text-mode anaconda installer
- **pykickstart**: Kickstart file processing  
- **Disk tools**: parted, gdisk, lvm2, cryptsetup
- **Filesystem tools**: e2fsprogs, xfsprogs, btrfs-progs
- **Container tools**: skopeo (bootc is in base image)

## Debugging

If installation fails, check the VM console output or journal:
```bash
# Run with console output
bcvk anaconda install --console ...

# Inside VM, check logs
journalctl -u bcvk-anaconda-setup
journalctl -u anaconda-direct
cat /tmp/anaconda.log
```
