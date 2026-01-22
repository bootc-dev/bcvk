# NAME

bcvk-ephemeral - Manage ephemeral VMs for bootc containers

# SYNOPSIS

**bcvk ephemeral** \<*SUBCOMMAND*\>

# DESCRIPTION

Manage stateless VMs for bootc container images.

Ephemeral VMs are managed by podman and boot directly from the container
image's filesystem. Unlike libvirt VMs created with **bcvk-libvirt-run**(8),
ephemeral VMs:

- **Boot directly from the container image** via virtiofs (no disk image creation)
- **Start in seconds** rather than minutes
- **Are stateless by default** - filesystem changes don't persist across restarts
- **Are managed via podman** - use familiar container tooling

This makes ephemeral VMs ideal for local development, CI pipelines, and any
workflow where you want fast iteration without persistent state.

## How It Works

Ephemeral VMs use a container-in-container architecture:

1. A podman container is launched containing QEMU and virtiofsd
2. The bootc container image's filesystem is shared into the VM via virtiofs
3. The VM boots using the kernel and initramfs from the container image
4. SSH access is provided via dynamically generated keys

The host needs /dev/kvm access and a virtualization stack (qemu, virtiofsd).

<!-- BEGIN GENERATED OPTIONS -->
<!-- END GENERATED OPTIONS -->

# SUBCOMMANDS

bcvk-ephemeral-run(8)

:   Run a bootc container as an ephemeral VM

bcvk-ephemeral-run-ssh(8)

:   Run an ephemeral VM and immediately SSH into it (auto-cleanup on exit)

bcvk-ephemeral-ssh(8)

:   SSH into a running ephemeral VM

bcvk-ephemeral-ps(8)

:   List running ephemeral VMs

bcvk-ephemeral-rm-all(8)

:   Remove all ephemeral VM containers

# EXAMPLES

## Quick Interactive Session

The simplest way to boot a bootc image is **run-ssh**, which starts the VM
and drops you into an SSH session. When you exit, the VM is cleaned up:

    bcvk ephemeral run-ssh quay.io/fedora/fedora-bootc:42

## Build and Test Workflow

A typical development loop combines podman build with ephemeral testing:

    # Edit your Containerfile
    vim Containerfile

    # Build the image
    podman build -t localhost/mybootc .

    # Test it immediately
    bcvk ephemeral run-ssh localhost/mybootc

    # Iterate: edit, build, test again
    podman build -t localhost/mybootc . && bcvk ephemeral run-ssh localhost/mybootc

## Background VM with SSH Access

For longer sessions or when you need to reconnect, run the VM in the background:

    # Start VM in background with SSH keys and auto-cleanup
    bcvk ephemeral run -d --rm -K --name testvm quay.io/fedora/fedora-bootc:42

    # SSH into it (can reconnect multiple times)
    bcvk ephemeral ssh testvm

    # Run commands directly
    bcvk ephemeral ssh testvm 'systemctl status'

    # Stop when done (--rm ensures cleanup)
    podman stop testvm

## Development VM with Host Directory Access

Mount your source code into the VM for development:

    bcvk ephemeral run -d --rm -K \
        --bind /home/user/project:project \
        --name devvm localhost/mybootc

    bcvk ephemeral ssh devvm
    # Inside VM: cd /run/virtiofs-mnt-project

## Resource Customization

Allocate more resources for heavy workloads:

    bcvk ephemeral run-ssh --memory 8G --vcpus 4 localhost/mybootc

Or use instance types:

    bcvk ephemeral run-ssh --itype u1.medium localhost/mybootc

# SEE ALSO

**bcvk**(8), **bcvk-ephemeral-run**(8), **bcvk-ephemeral-run-ssh**(8),
**bcvk-ephemeral-ssh**(8), **bcvk-libvirt**(8)

# VERSION

<!-- VERSION PLACEHOLDER -->
