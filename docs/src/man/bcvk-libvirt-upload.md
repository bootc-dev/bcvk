# NAME

bcvk-libvirt-upload - Upload bootc disk images to libvirt with metadata annotations

# SYNOPSIS

**bcvk libvirt upload** [*OPTIONS*]

# DESCRIPTION

Upload bootc disk images to libvirt with metadata annotations

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**SOURCE_IMAGE**

    Container image to install and upload

    This argument is required.

**--volume-name**=*VOLUME_NAME*

    Name for the libvirt volume (defaults to sanitized image name)

**--pool**=*POOL*

    Libvirt storage pool name

    Default: default

**--disk-size**=*DISK_SIZE*

    Size of the disk image (e.g., '20G', '10240M'). If not specified, uses the actual size of the created disk

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

**--memory**=*MEMORY*

    Memory size (e.g. 4G, 2048M, or plain number for MB)

    Default: 4G

**--vcpus**=*VCPUS*

    Number of vCPUs for installation VM

<!-- END GENERATED OPTIONS -->

# EXAMPLES

TODO: Add practical examples showing how to use this command.

# SEE ALSO

**bcvk**(8)

# VERSION

v0.1.0
