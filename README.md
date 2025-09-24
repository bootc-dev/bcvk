# bcvk - bootc virtualization kit

This project helps launch ephemeral VMs from bootc containers, and also create
disk images that can be imported into other virtualization frameworks.

## Installation

For now `git clone && cargo build --release`.

## Quick Start

### Running a bootc container as ephemeral VM 

This doesn't require any privileges, it's just a wrapper
for `podman`. It does require a virt stack (qemu, virtiofsd)
in the host environment.

```bash
bcvk ephemeral run -d --rm -K --name mytestvm quay.io/fedora/fedora-bootc:42
bcvk ephemeral ssh mytestvm
```

Or to fully streamline the above and have the VM automatically terminate when you exit
the SSH client:

```bash
bcvk ephemeral run-ssh quay.io/fedora/fedora-bootc:42
```

Everything with `bcvk ephemeral` creates a podman container that reuses the
host virtualization stack, and

### Creating a persistent bootable disk image from a container image
```bash
# Install bootc image to disk
bcvk to-disk quay.io/centos-bootc/centos-bootc:stream10 /path/to/disk.img
```

### Image management

There's a convenient helper function which filters by all container images
with the `containers.bootc=1` label: `bcvk images list`

### libvirt integration

The libvirt commands provide comprehensive integration with libvirt infrastructure for managing bootc containers as persistent VMs.

#### Running a bootc container as a libvirt VM

```bash
# Basic libvirt VM creation with default settings (2GB RAM, 2 CPUs, 20GB disk)
bcvk libvirt run quay.io/fedora/fedora-bootc:42

# Custom VM with specific resources and name
bcvk libvirt run --name my-fedora-vm --memory 4096 --cpus 4 --disk-size 50G quay.io/fedora/fedora-bootc:42

# Run VM with port forwarding and volume mounts
bcvk libvirt run --name web-server --port 8080:80 --volume /host/data:/mnt/data quay.io/centos-bootc/centos-bootc:stream10

# Run VM in background and automatically SSH into it
bcvk libvirt run --detach --ssh --name test-vm quay.io/fedora/fedora-bootc:42
```

#### Managing libvirt VMs

```bash
# List all bootc-related libvirt domains
bcvk libvirt list

# SSH into a running VM
bcvk libvirt ssh my-fedora-vm

# Stop a running VM
bcvk libvirt stop my-fedora-vm

# Start a stopped VM
bcvk libvirt start my-fedora-vm

# Get detailed information about a VM
bcvk libvirt inspect my-fedora-vm

# Remove a VM and its resources
bcvk libvirt rm my-fedora-vm
```

#### Advanced libvirt workflows

```bash
# Upload a pre-built disk image to libvirt storage
bcvk to-disk quay.io/fedora/fedora-bootc:42 /tmp/fedora.img
bcvk libvirt upload /tmp/fedora.img --name fedora-base

# Create a domain from uploaded image
bcvk libvirt create fedora-base --name my-vm --memory 8192

# Run with custom filesystem and network settings
bcvk libvirt run --filesystem xfs --network bridge quay.io/centos-bootc/centos-bootc:stream10
```

## Goals

This project aims to implement part of
<https://gitlab.com/fedora/bootc/tracker/-/issues/2>.

Basically it will be "bootc virtualization kit", and help users
run bootable containers as virtual machines.

Related projects and content:

- https://github.com/coreos/coreos-assembler/
- https://github.com/ublue-os/bluefin-lts/blob/main/Justfile

## Development

See [docs/HACKING.md](docs/HACKING.md).


