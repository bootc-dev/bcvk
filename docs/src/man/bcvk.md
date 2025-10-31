# NAME

bcvk - A toolkit for bootable containers and (local) virtualization.

# SYNOPSIS

**bcvk** \[**-h**\|**\--help**\] \<*subcommands*\>

# DESCRIPTION

bcvk helps launch bootc containers using local virtualization.
Build containers using your tool of choice (podman, docker, etc),
then use `bcvk libvirt run` to quickly and conveniently create
a libvirt virtual machine, and connect with `ssh`.

The toolkit includes commands for:

- Project-scoped VM management with Vagrant-like workflow
- Running ephemeral VMs for testing container images
- Installing bootc containers to persistent disk images
- Managing libvirt integration and VM lifecycle
- Executing host commands from within containers
- SSH access to running VMs

<!-- BEGIN GENERATED OPTIONS -->
<!-- END GENERATED OPTIONS -->

# SUBCOMMANDS

bcvk-hostexec(8)

:   Execute commands on the host system from within containers

bcvk-project(8)

:   Project-scoped VM management (Vagrant-like workflow)

bcvk-images(8)

:   Manage and inspect bootc container images

bcvk-run-ephemeral(8)

:   Run bootc containers as temporary VMs for testing and development

bcvk-to-disk(8)

:   Install bootc images to persistent disk images

bcvk-libvirt(8)

:   Manage libvirt integration for bootc containers

bcvk-ssh(8)

:   Connect to running VMs via SSH

bcvk-help(8)

:   Print this message or the help of the given subcommand(s)

# VERSION

v0.1.0
