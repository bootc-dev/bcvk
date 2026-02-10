# NAME

bcvk - A toolkit for bootable containers and (local) virtualization.

# SYNOPSIS

**bcvk** \[**-h**\|**\--help**\] \<*subcommands*\>

# DESCRIPTION

bcvk helps launch bootc containers using local virtualization for
development and CI workflows. Build containers using your tool of choice
(podman, docker, etc), then use bcvk to boot them as virtual machines.

Note: bcvk is designed for local development and CI environments, not for
running production servers.

## Quick Start

The fastest way to boot a bootc container image is with **bcvk ephemeral run-ssh**:

    bcvk ephemeral run-ssh quay.io/fedora/fedora-bootc:42

This boots the container as a VM and drops you into an SSH session. When you
exit the SSH session, the VM is automatically cleaned up.

## Build and Test Workflow

A typical development workflow combines container builds with ephemeral VM testing:

    # Build your bootc container
    podman build -t localhost/myimage .

    # Boot it as a VM and SSH in (auto-cleanup on exit)
    bcvk ephemeral run-ssh localhost/myimage

    # Or run in background for longer testing
    bcvk ephemeral run -d --rm -K --name testvm localhost/myimage
    bcvk ephemeral ssh testvm
    # ... test, then stop the VM when done
    podman stop testvm

## Ephemeral vs Libvirt

bcvk provides two ways to run bootc containers as VMs:

**bcvk ephemeral** runs stateless VMs managed by podman. The VM boots directly
from the container image's filesystem via virtiofs with no disk image creation,
making startup very fast. Ideal for quick iteration and CI pipelines.

**bcvk libvirt** creates stateful VMs managed by libvirt with persistent disk
images. These VMs survive reboots and support the full bootc upgrade workflow.
Useful for longer-running local development or testing upgrade scenarios.

<!-- BEGIN GENERATED OPTIONS -->
<!-- END GENERATED OPTIONS -->

# SUBCOMMANDS

bcvk-ephemeral(8)

:   Manage stateless VMs via podman (fast startup, no disk images)

bcvk-images(8)

:   Manage and inspect bootc container images

bcvk-to-disk(8)

:   Install bootc images to persistent disk images

bcvk-anaconda(8)

:   Install bootc images using anaconda and kickstart files

bcvk-libvirt(8)

:   Manage stateful VMs via libvirt (persistent disk images)

# EXAMPLES

Test a public bootc image interactively:

    bcvk ephemeral run-ssh quay.io/centos-bootc/centos-bootc:stream10

Build and test a local image:

    podman build -t localhost/mybootc .
    bcvk ephemeral run-ssh localhost/mybootc

Run a background VM with SSH access:

    bcvk ephemeral run -d --rm -K --name dev quay.io/fedora/fedora-bootc:42
    bcvk ephemeral ssh dev

Create a libvirt VM with persistent disk:

    bcvk libvirt run --name myvm quay.io/centos-bootc/centos-bootc:stream10
    bcvk libvirt ssh myvm

# SEE ALSO

**bcvk-ephemeral**(8), **bcvk-ephemeral-run**(8), **bcvk-ephemeral-run-ssh**(8),
**bcvk-libvirt**(8), **bcvk-to-disk**(8), **bootc**(8)

# VERSION

<!-- VERSION PLACEHOLDER -->
