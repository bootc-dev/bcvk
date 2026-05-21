# Quick Start

## Prerequisites

- bcvk installed (see [Installation Guide](./installation.md))
- podman
- QEMU/KVM
- A bootc container image

## Your First VM

```bash
bcvk ephemeral run-ssh quay.io/fedora/fedora-bootc:42
```

This starts a VM and automatically SSHs into it. The VM terminates when you exit the SSH session.

## Ephemeral VMs

```bash
# Start a background VM with auto-cleanup
bcvk ephemeral run -d --rm -K --name mytestvm quay.io/fedora/fedora-bootc:42

# SSH into it
bcvk ephemeral ssh mytestvm
```

## Creating Disk Images

```bash
# Raw disk image
bcvk to-disk quay.io/centos-bootc/centos-bootc:stream10 /path/to/disk.img

# qcow2 format
bcvk to-disk --format qcow2 quay.io/fedora/fedora-bootc:42 /path/to/fedora.qcow2

# Custom size
bcvk to-disk --disk-size 20G quay.io/fedora/fedora-bootc:42 /path/to/large-disk.img
```

## Persistent VMs with libvirt

```bash
# Create and start
bcvk libvirt run --name my-server quay.io/fedora/fedora-bootc:42

# Manage lifecycle
bcvk libvirt ssh my-server
bcvk libvirt stop my-server
bcvk libvirt start my-server
bcvk libvirt list
bcvk libvirt rm my-server
```

## Image Management

```bash
# List bootc images
bcvk images list
```

## Resource Configuration

```bash
# Ephemeral VM
bcvk ephemeral run --memory 4096 --cpus 4 --name bigvm quay.io/fedora/fedora-bootc:42

# libvirt VM
bcvk libvirt run --name webserver --memory 8192 --cpus 8 --disk-size 50G quay.io/centos-bootc/centos-bootc:stream10
```