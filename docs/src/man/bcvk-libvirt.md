# NAME

bcvk-libvirt - Manage libvirt integration for bootc containers

# SYNOPSIS

**bcvk libvirt** \[**-h**\|**\--help**\] \<*subcommands*\>

**bcvk v** \[**-h**\|**\--help**\] \<*subcommands*\>

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

bcvk-libvirt-upload(8)

:   Upload bootc disk images to libvirt storage pools

bcvk-libvirt-create(8)

:   Create libvirt domains from bootc disk images

bcvk-libvirt-list(8)

:   List bootc-related libvirt domains and storage

bcvk-libvirt-help(8)

:   Print this message or the help of the given subcommand(s)

# VERSION

v0.1.0