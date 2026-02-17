# NAME

bcvk-libvirt - Manage libvirt integration for bootc containers

# SYNOPSIS

**bcvk libvirt** \[**-h**\|**\--help**\] \<*subcommands*\>

# DESCRIPTION

Comprehensive libvirt integration with subcommands for uploading disk images,
creating domains, and managing bootc containers as libvirt VMs.

This command provides seamless integration between bcvk disk images and
libvirt virtualization infrastructure, enabling:

- Upload of disk images to libvirt storage pools
- Creation of libvirt domains with appropriate bootc annotations
- Management of VM lifecycle through libvirt
- Integration with existing libvirt-based infrastructure

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**-c**, **--connect**=*CONNECT*

    Hypervisor connection URI (e.g., qemu:///system, qemu+ssh://host/system)

<!-- END GENERATED OPTIONS -->

# SUBCOMMANDS

bcvk-libvirt-run(8)

:   Run a bootable container as a persistent VM

bcvk-libvirt-run-anaconda(8)

:   Run a bootable container as a persistent VM, installed via anaconda

bcvk-libvirt-upload(8)

:   Upload bootc disk images to libvirt storage pools

bcvk-libvirt-create(8)

:   Create libvirt domains from bootc disk images

bcvk-libvirt-list(8)

:   List bootc-related libvirt domains and storage

bcvk-libvirt-ssh(8)

:   SSH into a libvirt VM

bcvk-libvirt-stop(8)

:   Stop a libvirt VM

bcvk-libvirt-start(8)

:   Start a stopped libvirt VM

bcvk-libvirt-rm(8)

:   Remove a libvirt VM

bcvk-libvirt-inspect(8)

:   Show detailed information about a libvirt VM

bcvk-libvirt-help(8)

:   Print this message or the help of the given subcommand(s)

# VERSION

v0.1.0