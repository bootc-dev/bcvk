# bcvk - bootc virtualization kit

This project helps launch ephemeral VMs from bootc containers, and also create
disk images that can be imported into other virtualization frameworks.

## Installation

See [docs/src/installation.md](./docs/src/installation.md).

## Dependencies

bcvk requires the following runtime dependencies:

### Core virtualization
- **QEMU** - The core virtualization engine
- **virtiofsd** - VirtIO filesystem daemon for sharing directories with VMs
- **podman** - Container runtime for managing bootc images

### For libvirt integration
- **libvirt** - Virtualization management for persistent VMs

### For cloud-init support
- **dosfstools** - Provides `mkfs.vfat` for creating VFAT filesystems
- **mtools** - Provides `mcopy` for populating VFAT images (used for cloud-init ConfigDrive)

### Package installation

**Debian/Ubuntu:**
```bash
sudo apt install qemu-kvm qemu-system qemu-utils virtiofsd podman libvirt-daemon libvirt-clients dosfstools mtools
```

**Fedora/RHEL:**
```bash
sudo dnf install qemu-kvm qemu-img virtiofsd podman libvirt libvirt-client dosfstools mtools
```

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
host virtualization stack, making it simple to test bootc containers without
requiring root privileges or dedicated VM infrastructure.

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

#### Starting a bootc container as a libvirt VM

```bash
# Basic libvirt VM creation with default settings (2GB RAM, 2 CPUs, 20GB disk)
bcvk libvirt run quay.io/centos-bootc/centos-bootc:stream10

# Note requirement for --filesystem with the generic Fedora bootc base images
bcvk libvirt run --filesystem btrfs quay.io/fedora/fedora-bootc:43

# Custom VM with specific resources and name
bcvk libvirt run --name example-vm --memory 4096 --cpus 4 --disk-size 50G quay.io/centos-bootc/centos-bootc:stream10

# This example forwards a port and bind mounts content from the host
bcvk libvirt run --name web-server --port 8080:80 --volume /host/data:/mnt/data localhost/myimage

# Bind mount the host container storage for faster updates
bcvk libvirt run --update-from-host --name devvm localhost/myimage
```

#### Using and managing libvirt VMs

After initializing a VM a common next step is `bcvk lbivirt ssh <vm name>`.
bcvk defaults to injecting SSH keys via [systemd credentials](https://systemd.io/CREDENTIALS/).
The private key is specific to the VM and is stored in the domain metadata.

Other operations:

```bash
# List all bootc-related libvirt domains
bcvk libvirt list

# Stop a running VM
bcvk libvirt stop my-fedora-vm

# Start a stopped VM
bcvk libvirt start my-fedora-vm

# Get detailed information about a VM
bcvk libvirt inspect my-fedora-vm

# Remove a VM and its resources
bcvk libvirt rm -f my-fedora-vm
```

## Other operations

The `bcvk libvirt run` command wraps `bcvk to-disk` which in
turns wraps `bootc install to-disk` in an ephemeral VM. In
some cases, you may want to create a disk image directly.

```bash
# Generate a disk image in qcow2 format.
bcvk to-disk --format=qcow2 localhost/my-container-image output-disk.qcow2
```

Note that at the current time, this project is not scoped to
output other virtualization formats. The [bootc image builder](https://github.com/osbuild/bootc-image-builder)
is one project that offers those.

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


