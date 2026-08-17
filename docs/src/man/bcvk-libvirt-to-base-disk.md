# NAME

bcvk-libvirt-to-base-disk - Create a base disk image for libvirt VMs

# SYNOPSIS

**bcvk libvirt to-base-disk** [*OPTIONS*]

# DESCRIPTION

Create a base disk image for libvirt VMs

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**SOURCE_IMAGE**

    This argument is required.

**--image-to-install**=*IMAGE_TO_INSTALL*

    The image to use for creating the base disk If None, the `source_image` cli option is used for installation

**--filesystem**=*FILESYSTEM*

    Root filesystem type (e.g. ext4, xfs, btrfs)

**--root-size**=*ROOT_SIZE*

    Root filesystem size (e.g., '10G', '5120M')

**--storage-path**=*STORAGE_PATH*

    Path to host container storage (auto-detected if not specified)

**--target-transport**=*TARGET_TRANSPORT*

    The transport; e.g. oci, oci-archive, containers-storage.  Defaults to `registry`

**--karg**=*KARG*

    Set a kernel argument

**--composefs-backend**

    Default to composefs-native storage

**--bootloader**=*BOOTLOADER*

    Which bootloader to use for composefs-native backend

**--allow-missing-fsverity**

    Allow installation without fs-verity support for composefs-native backend

<!-- END GENERATED OPTIONS -->

# EXAMPLES

By default, a base disk is created transparently when `bcvk libvirt run` is used. However, you can
use this command to ensure a disk is generated in advance of launching multiple concurrent VMs.

Create a base disk:

    bcvk libvirt to-base-disk --filesystem=ext4 --root-size=10G quay.io/fedora/fedora-bootc:latest


# SEE ALSO

**bcvk**(8)

# VERSION

<!-- VERSION PLACEHOLDER -->
