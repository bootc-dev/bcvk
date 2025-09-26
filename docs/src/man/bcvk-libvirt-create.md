# NAME

bcvk-libvirt-create - Create and start domains from uploaded bootc volumes

# SYNOPSIS

**bcvk libvirt create** [*OPTIONS*]

# DESCRIPTION

Create and start domains from uploaded bootc volumes

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**VOLUME_NAME_OR_IMAGE**

    Name of the bootc volume to use for domain creation, OR container image to create from If this looks like a container image (contains '/' or ':'), will automatically upload if needed

    This argument is required.

**--pool**=*POOL*

    Libvirt storage pool name

    Default: default

**--domain-name**=*DOMAIN_NAME*

    Name for the libvirt domain (defaults to volume name)

**--memory**=*MEMORY*

    Memory size for the domain (e.g. 2G, 1024M)

    Default: 4G

**--vcpus**=*VCPUS*

    Number of vCPUs for the domain

**--network**=*NETWORK*

    Network configuration (default, bridge=name, none)

    Default: default

**--start**

    Start the domain after creation

**--vnc**

    Enable VNC console access

**--vnc-port**=*VNC_PORT*

    VNC port (default: auto-assign)

**--karg**=*KARG*

    Additional kernel arguments

**--dry-run**

    Dry run - validate configuration without creating domain

**--force**

    Force creation even if domain already exists

**--filesystem**=*FILESYSTEM*

    Root filesystem type (e.g. ext4, xfs, btrfs)

**--root-size**=*ROOT_SIZE*

    Root filesystem size (e.g., '10G', '5120M')

**--storage-path**=*STORAGE_PATH*

    Path to host container storage (auto-detected if not specified)

**--disk-size**=*DISK_SIZE*

    Size of the disk image for automatic upload (e.g., '20G', '10240M')

**--memory**=*MEMORY*

    Memory size (e.g. 4G, 2048M, or plain number for MB)

    Default: 4G

**--install-vcpus**=*INSTALL_VCPUS*

    Number of vCPUs for installation VM during auto-upload

**--generate-ssh-key**

    Generate ephemeral SSH keypair and inject into domain

**--ssh-key**=*SSH_KEY*

    Path to existing SSH private key to use (public key must exist at <key>.pub)

**--ssh-port**=*SSH_PORT*

    SSH port for port forwarding (default: auto-assign)

<!-- END GENERATED OPTIONS -->

# EXAMPLES

TODO: Add practical examples showing how to use this command.

# SEE ALSO

**bcvk**(8)

# VERSION

v0.1.0
