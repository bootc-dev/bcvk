# NAME

bcvk-libvirt-run - Run a bootable container as a persistent VM

# SYNOPSIS

**bcvk libvirt run** [*OPTIONS*]

# DESCRIPTION

Run a bootable container as a persistent VM

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**IMAGE**

    Container image to run as a bootable VM

    This argument is required.

**--name**=*NAME*

    Name for the VM (auto-generated if not specified)

**--memory**=*MEMORY*

    Memory size (e.g. 4G, 2048M, or plain number for MB)

    Default: 4G

**--cpus**=*CPUS*

    Number of virtual CPUs for the VM

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

**--karg**=*KARG*

    Set a kernel argument

**--composefs-native**

    Default to composefs-native storage

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

**--lifecycle-bind-parent**

    Bind VM lifecycle to parent process (shutdown VM when parent exits)

<!-- END GENERATED OPTIONS -->

# EXAMPLES

Create and start a persistent VM:

    bcvk libvirt run --name my-server quay.io/fedora/fedora-bootc:42

Create a VM with custom resources:

    bcvk libvirt run --name webserver --memory 8192 --cpus 8 --disk-size 50G quay.io/centos-bootc/centos-bootc:stream10

Create a VM with port forwarding:

    bcvk libvirt run --name webserver --port 8080:80 quay.io/centos-bootc/centos-bootc:stream10

Create a VM with volume mount:

    bcvk libvirt run --name devvm --volume /home/user/code:/workspace quay.io/fedora/fedora-bootc:42

Create a VM and automatically SSH into it:

    bcvk libvirt run --name testvm --ssh quay.io/fedora/fedora-bootc:42

Create a VM with access to host container storage for bootc upgrade:

    bcvk libvirt run --name upgrade-test --bind-storage-ro quay.io/fedora/fedora-bootc:42

Server management workflow:

    # Create a persistent server VM
    bcvk libvirt run --name production-server --memory 8192 --cpus 4 --disk-size 100G my-server-image
    
    # Check status
    bcvk libvirt list
    
    # Access for maintenance
    bcvk libvirt ssh production-server

# SEE ALSO

**bcvk**(8)

# VERSION

v0.1.0
