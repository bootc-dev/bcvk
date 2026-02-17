# NAME

bcvk-anaconda-install - Install a bootc container to disk using anaconda

# SYNOPSIS

**bcvk anaconda install** \[*OPTIONS*\] **-k** *KICKSTART* *IMAGE* *TARGET_DISK*

# DESCRIPTION

Install a bootc container image to a disk image using anaconda as the
installation engine. The user provides a kickstart file with partitioning
and system configuration; bcvk automatically injects the **ostreecontainer**
directive to pull the image from host container storage.

The installation runs inside an ephemeral VM. The host's container storage
is mounted read-only via virtiofs, allowing anaconda to access local images
without copying. When installation completes, the VM powers off and the
disk image is ready for use.

## Kickstart Requirements

Your kickstart file must include:

- **Partitioning**: Use **autopart**, **part**, or other partitioning commands
- **Target disk**: Use `ignoredisk --only-use=/dev/disk/by-id/virtio-output`
  (bcvk also attaches a swap disk that should be ignored)
- **Boot partitions**: Use `reqpart --add-boot` for BIOS/UEFI boot partitions
- **Poweroff**: Include `poweroff` so the VM exits when done

Do **not** include an **ostreecontainer** directive; bcvk injects this
automatically with the correct transport.

## Registry Repointing

By default, bcvk injects a **%post** script that runs `bootc switch` to
repoint the installed system's origin to the registry. This ensures that
`bootc upgrade` pulls updates from the registry rather than expecting
containers-storage (which won't exist on the installed system).

Use **--no-repoint** if you want to handle this yourself, or if the image
will only be used locally.

Use **--target-imgref** to specify a different registry reference than the
source image (e.g., when installing from a local build but wanting updates
from a production registry).

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**IMAGE**

    Bootc container image to install (from host container storage)

    This argument is required.

**TARGET_DISK**

    Target disk image file path

    This argument is required.

**-k**, **--kickstart**=*KICKSTART*

    Kickstart file with partitioning and system configuration

**--target-imgref**=*TARGET_IMGREF*

    Target image reference for the installed system

**--no-repoint**

    Skip injecting the %post script that repoints to target-imgref

**--anaconda-image**=*ANACONDA_IMAGE*

    Anaconda container image to use as the installer

    Default: localhost/anaconda-bootc:latest

**--disk-size**=*DISK_SIZE*

    Disk size to create (e.g. 10G, 5120M)

**--format**=*FORMAT*

    Output disk image format

    Possible values:
    - raw
    - qcow2

    Default: raw

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

<!-- END GENERATED OPTIONS -->

# EXAMPLES

Basic installation with a minimal kickstart:

    cat > my.ks << 'EOF'
    text
    lang en_US.UTF-8
    keyboard us
    timezone UTC --utc
    network --bootproto=dhcp --activate

    ignoredisk --only-use=/dev/disk/by-id/virtio-output
    zerombr
    clearpart --all --initlabel
    reqpart --add-boot
    part / --fstype=xfs --grow

    rootpw --lock
    poweroff
    EOF

    bcvk anaconda install -k my.ks --disk-size 20G \
        quay.io/fedora/fedora-bootc:42 output.img

Install a locally-built image, repointing to production registry:

    podman build -t localhost/myapp:dev .
    bcvk anaconda install -k prod.ks --disk-size 50G \
        --target-imgref registry.example.com/myapp:latest \
        localhost/myapp:dev production.img

Create a qcow2 disk image for use with libvirt:

    bcvk anaconda install -k server.ks --disk-size 100G --format qcow2 \
        quay.io/centos-bootc/centos-bootc:stream10 server.qcow2

Debug installation issues with console output:

    bcvk anaconda install -k my.ks --disk-size 20G --console \
        localhost/myimage:latest debug.img

# KICKSTART EXAMPLE

A complete kickstart for BIOS boot with LVM:

    text
    lang en_US.UTF-8
    keyboard us
    timezone America/New_York --utc
    network --bootproto=dhcp --device=link --activate

    # Target the bcvk output disk
    ignoredisk --only-use=/dev/disk/by-id/virtio-output

    zerombr
    clearpart --all --initlabel

    # Create boot partitions (biosboot + /boot)
    reqpart --add-boot

    # LVM layout
    part pv.01 --grow
    volgroup vg0 pv.01
    logvol / --vgname=vg0 --name=root --fstype=xfs --size=10240
    logvol /var --vgname=vg0 --name=var --fstype=xfs --size=5120 --grow

    rootpw --lock
    poweroff

# SEE ALSO

**bcvk-anaconda**(8), **bcvk-to-disk**(8), **bcvk**(8), **bootc**(8)

# VERSION

<!-- VERSION PLACEHOLDER -->
