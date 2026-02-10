# NAME

bcvk-libvirt-run-anaconda - Run a bootable container as a persistent VM, installed via anaconda

# SYNOPSIS

**bcvk libvirt run-anaconda** [*OPTIONS*] **--kickstart** *KICKSTART* *IMAGE*

# DESCRIPTION

Run a bootable container as a persistent VM using anaconda for installation.
This command is similar to `bcvk libvirt run`, but uses anaconda with kickstart
files instead of `bootc install to-disk` for the installation phase.

This allows for more flexible partitioning schemes and system configuration
through kickstart files, while still providing the same VM lifecycle management
(SSH access, networking, bind mounts, etc.) as `bcvk libvirt run`.

The `ostreecontainer` directive is injected automatically into the kickstart
file, so you only need to provide the partitioning and system configuration.

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**IMAGE**

    Container image to run as a bootable VM

    This argument is required.

**-k**, **--kickstart**=*KICKSTART*

    Kickstart file with partitioning and system configuration

**--name**=*NAME*

    Name for the VM (auto-generated if not specified)

**-R**, **--replace**

    Replace existing VM with same name (stop and remove if exists)

**--target-imgref**=*TARGET_IMGREF*

    Target image reference for the installed system

**--no-repoint**

    Skip injecting the %post script that repoints to target-imgref

**--anaconda-image**=*ANACONDA_IMAGE*

    Anaconda container image to use as the installer

    Default: localhost/anaconda-bootc:latest

**--itype**=*ITYPE*

    Instance type (e.g., u1.nano, u1.small, u1.medium). Overrides cpus/memory if specified.

**--memory**=*MEMORY*

    Memory size (e.g. 4G, 2048M, or plain number for MB)

    Default: 4G

**--cpus**=*CPUS*

    Number of virtual CPUs for the VM (overridden by --itype if specified)

    Default: 2

**--disk-size**=*DISK_SIZE*

    Disk size for the VM (e.g. 20G, 10240M, or plain number for bytes)

    Default: 20G

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

**-p**, **--port**=*PORT_MAPPINGS*

    Port mapping from host to VM (format: host_port:guest_port, e.g., 8080:80)

**-v**, **--volume**=*RAW_VOLUMES*

    Volume mount from host to VM (raw virtiofs tag, for manual mounting)

**--bind**=*BIND_MOUNTS*

    Bind mount from host to VM (format: host_path:guest_path)

**--bind-ro**=*BIND_MOUNTS_RO*

    Bind mount from host to VM as read-only (format: host_path:guest_path)

**--network**=*NETWORK*

    Network mode for the VM

    Default: user

**--detach**

    Keep the VM running in background after creation

**--ssh**

    Automatically SSH into the VM after creation

**--ssh-wait**

    Wait for SSH to become available and verify connectivity (for testing)

**--bind-storage-ro**

    Mount host container storage (RO) at /run/host-container-storage

**--firmware**=*FIRMWARE*

    Firmware type for the VM (defaults to uefi-secure)

    Possible values:
    - uefi-secure
    - uefi-insecure
    - bios

    Default: uefi-secure

**--disable-tpm**

    Disable TPM 2.0 support (enabled by default)

**--secure-boot-keys**=*SECURE_BOOT_KEYS*

    Directory containing secure boot keys (required for uefi-secure)

**--label**=*LABEL*

    User-defined labels for organizing VMs (comma not allowed in labels)

**--transient**

    Create a transient VM that disappears on shutdown/reboot

<!-- END GENERATED OPTIONS -->

# KICKSTART FILE

The kickstart file should contain partitioning and system configuration.
You do NOT need to include the `ostreecontainer` directive - bcvk injects
this automatically with the correct transport.

Example minimal kickstart:

```
text
lang en_US.UTF-8
keyboard us
timezone UTC --utc
network --bootproto=dhcp --activate

zerombr
clearpart --all --initlabel
reqpart --add-boot
autopart --type=plain --fstype=xfs
bootloader --location=mbr
rootpw --lock

poweroff
```

# EXAMPLES

Create a VM using anaconda with a kickstart file:

    bcvk libvirt run-anaconda --name my-server \
        --kickstart my-config.ks \
        quay.io/fedora/fedora-bootc:42

Create a VM with custom resources and SSH access:

    bcvk libvirt run-anaconda --name webserver \
        --kickstart server.ks \
        --memory 8192 --cpus 8 --disk-size 50G \
        --ssh \
        quay.io/centos-bootc/centos-bootc:stream10

Create a VM with a custom target image reference:

    bcvk libvirt run-anaconda --name prod-server \
        --kickstart prod.ks \
        --target-imgref registry.example.com/myapp:prod \
        quay.io/fedora/fedora-bootc:42

Test anaconda installation workflow:

    # Build the anaconda-bootc container first
    podman build -t localhost/anaconda-bootc:latest containers/anaconda-bootc/
    
    # Create a VM with anaconda installation
    bcvk libvirt run-anaconda --name test-vm \
        --kickstart test.ks \
        --ssh-wait \
        quay.io/fedora/fedora-bootc:42

# SEE ALSO

**bcvk-libvirt-run**(8), **bcvk-anaconda-install**(8), **bcvk**(8)

# VERSION

v0.1.0
