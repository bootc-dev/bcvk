# NAME

bcvk-to-disk - Install bootc images to persistent disk images

# SYNOPSIS

**bcvk to-disk** \[**-h**\|**\--help**\] \[*OPTIONS*\] *IMAGE*

# DESCRIPTION

Performs automated installation of bootc containers to disk images
using ephemeral VMs as the installation environment. Supports multiple
filesystems, custom sizing, and creates bootable disk images ready
for production deployment.

The installation process:

1. Creates a new disk image with the specified filesystem layout
2. Boots an ephemeral VM with the target container image
3. Runs \`bootc install to-disk\` within the VM to install to the disk
4. Produces a bootable disk image that can be deployed anywhere

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**SOURCE_IMAGE**

    Container image to install

    This argument is required.

**TARGET_DISK**

    Target disk/device path

    This argument is required.

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

**--disk-size**=*DISK_SIZE*

    Disk size to create (e.g. 10G, 5120M, or plain number for bytes)

**--format**=*FORMAT*

    Output disk image format

    Possible values:
    - raw
    - qcow2

    Default: raw

**--itype**=*ITYPE*

    Instance type (e.g., u1.nano, u1.small, u1.medium). Overrides vcpus/memory if specified.

**--memory**=*MEMORY*

    Memory size (e.g. 4G, 2048M, or plain number for MB)

    Default: 4G

**--vcpus**=*VCPUS*

    Number of vCPUs (overridden by --itype if specified)

**--console**

    Enable console output to terminal for debugging

**--debug**

    Enable debug mode (drop to shell instead of running QEMU)

**--virtio-serial-out**=*NAME:FILE*

    Add virtio-serial device with output to file (format: name:/path/to/file)

**--execute**=*EXECUTE*

    Execute command inside VM via systemd and capture output

**-K**, **--ssh-keygen**

    Generate SSH keypair and inject via systemd credentials

**--install-log**=*INSTALL_LOG*

    Configure logging for `bootc install` by setting the `RUST_LOG` environment variable

**--label**=*LABEL*

    Add metadata to the container in key=value form

**--dry-run**

    Check if the disk would be regenerated without actually creating it

<!-- END GENERATED OPTIONS -->

# ARGUMENTS

*IMAGE*

:   Container image reference to install (e.g., \`registry.example.com/my-bootc:latest\`)

# EXAMPLES

Create a raw disk image:

    bcvk to-disk quay.io/centos-bootc/centos-bootc:stream10 /path/to/disk.img

Create a qcow2 disk image (more compact):

    bcvk to-disk --format qcow2 quay.io/fedora/fedora-bootc:42 /path/to/fedora.qcow2

Create with specific disk size:

    bcvk to-disk --disk-size 20G quay.io/fedora/fedora-bootc:42 /path/to/large-disk.img

Create with custom filesystem and root size:

    bcvk to-disk --filesystem btrfs --root-size 15G quay.io/fedora/fedora-bootc:42 /path/to/btrfs-disk.img

Development workflow - test then create deployment image:

    # Test the container as a VM first
    bcvk ephemeral run-ssh my-app
    
    # If good, create the deployment image
    bcvk to-disk my-app /tmp/my-app.img

# VERSION

v0.1.0