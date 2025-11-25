# NAME

bcvk-ephemeral-run-ssh - Run ephemeral VM and SSH into it

# SYNOPSIS

**bcvk ephemeral run-ssh** [*OPTIONS*]

# DESCRIPTION

Run ephemeral VM and SSH into it

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**IMAGE**

    Container image to run as ephemeral VM

    This argument is required.

**SSH_ARGS**

    SSH command to execute (optional, defaults to interactive shell)

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

**-t**, **--tty**

    Allocate a pseudo-TTY for container

**-i**, **--interactive**

    Keep STDIN open for container

**-d**, **--detach**

    Run container in background

**--rm**

    Automatically remove container when it exits

**--name**=*NAME*

    Assign a name to the container

**--network**=*NETWORK*

    Configure the network for the container

**--label**=*LABEL*

    Add metadata to the container in key=value form

**-e**, **--env**=*ENV*

    Set environment variables in the container (key=value)

**--bind**=*HOST_PATH[:NAME]*

    Bind mount host directory (RW) at /run/virtiofs-mnt-<name>

**--ro-bind**=*HOST_PATH[:NAME]*

    Bind mount host directory (RO) at /run/virtiofs-mnt-<name>

**--systemd-units**=*SYSTEMD_UNITS_DIR*

    Directory with systemd units to inject (expects system/ subdirectory)

**--bind-storage-ro**

    Mount host container storage (RO) at /run/virtiofs-mnt-hoststorage

**--add-swap**=*ADD_SWAP*

    Allocate a swap device of the provided size

**--mount-disk-file**=*FILE[:NAME]*

    Mount disk file as virtio-blk device at /dev/disk/by-id/virtio-<name>

**--karg**=*KERNEL_ARGS*

    Additional kernel command line arguments

**--cloud-init**=*PATH*

    Path to cloud-config file (user-data) for cloud-init ConfigDrive

**--cloud-init-empty**

    Create an empty cloud-init ConfigDrive (no custom user-data)

<!-- END GENERATED OPTIONS -->

# EXAMPLES

Run an ephemeral VM and automatically SSH into it (VM cleans up when SSH exits):

    bcvk ephemeral run-ssh quay.io/fedora/fedora-bootc:42

Run a quick test with automatic SSH and cleanup:

    bcvk ephemeral run-ssh quay.io/fedora/fedora-bootc:42

Execute a specific command via SSH:

    bcvk ephemeral run-ssh quay.io/fedora/fedora-bootc:42 'systemctl status'

Run with custom memory and CPU allocation:

    bcvk ephemeral run-ssh --memory 8G --vcpus 4 quay.io/fedora/fedora-bootc:42

# SEE ALSO

**bcvk**(8)

# VERSION

<!-- VERSION PLACEHOLDER -->
