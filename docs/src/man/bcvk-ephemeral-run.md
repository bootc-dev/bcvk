# NAME

bcvk-ephemeral-run - Run bootc containers as ephemeral VMs

# SYNOPSIS

**bcvk ephemeral run** [*OPTIONS*]

# DESCRIPTION

Run bootc containers as ephemeral VMs using a sophisticated container-in-container approach.

## How It Works

This command creates an ephemeral virtual machine by launching a podman container that contains and runs QEMU. The process works as follows:

1. **Container Setup**: A privileged podman container is launched with access to the host's virtualization infrastructure
2. **Host Virtualization Access**: The container gains access to:
   - `/dev/kvm` for hardware virtualization
   - Host's virtiofsd daemon for filesystem sharing
   - QEMU binaries and virtualization stack
3. **VM Creation**: Inside the container, QEMU is executed to create a virtual machine
4. **Root Filesystem**: The bootc container image's root filesystem becomes the VM's root filesystem, mounted via virtiofs
5. **Kernel Boot**: The VM boots using the kernel and initramfs from the bootc container image

This architecture provides several advantages:
- **No Root Required**: Runs as a regular user without requiring root privileges on the host
- **Isolation**: The VM runs in a contained environment separate from the host
- **Fast I/O**: virtiofs provides efficient filesystem access between container and VM
- **Resource Efficiency**: Leverages existing container infrastructure while providing full VM capabilities

## Container-VM Relationship

The relationship between the podman container and the VM inside it:

- **Podman Container**: Acts as the virtualization environment, providing QEMU and system services
- **QEMU Process**: Runs inside the podman container, creating the actual virtual machine
- **VM Guest**: The bootc container image runs as a complete operating system inside the VM
- **Filesystem Sharing**: The container's root filesystem is shared with the VM via virtiofs at runtime

This design allows bcvk to provide VM-like isolation and boot behavior while leveraging container tooling and not requiring root access on the host system.

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**IMAGE**

    Container image to run as ephemeral VM

    This argument is required.

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

Run an ephemeral VM in the background:

    bcvk ephemeral run -d --rm --name mytestvm quay.io/fedora/fedora-bootc:42

Run with custom memory and CPU allocation:

    bcvk ephemeral run --memory 8G --vcpus 4 --name bigvm quay.io/fedora/fedora-bootc:42

Run with automatic SSH key generation and removal when done:

    bcvk ephemeral run -d --rm -K --name testvm quay.io/fedora/fedora-bootc:42

Run with host directory bind mount:

    bcvk ephemeral run --bind /home/user/code:workspace --name devvm quay.io/fedora/fedora-bootc:42

Run with console output for debugging:

    bcvk ephemeral run --console --name debugvm quay.io/fedora/fedora-bootc:42

Run with custom kernel arguments:

    bcvk ephemeral run --karg "console=ttyS0" --name serialvm quay.io/fedora/fedora-bootc:42

Development workflow example:

    # Start a development VM with code mounted
    bcvk ephemeral run -d --rm -K --bind /home/user/project:code --name devvm quay.io/fedora/fedora-bootc:42
    
    # SSH into it for development
    bcvk ephemeral ssh devvm
    
    # VM automatically cleans up when stopped due to --rm flag

# DEBUGGING

When troubleshooting ephemeral VM issues, bcvk provides several debugging logs that can be accessed from within the container.

## Guest Journal Log

The systemd journal from the guest VM is automatically streamed to `/run/journal.log` inside the container. This log captures all boot messages, service startup events, and system errors from the VM's perspective.

To view the journal log:

    # For a running detached VM
    podman exec <container-id> tail -f /run/journal.log

    # View specific systemd service messages
    podman exec <container-id> grep "dbus-broker" /run/journal.log

    # Save journal for offline analysis
    podman exec <container-id> cat /run/journal.log > guest-journal.log

The journal log is particularly useful for:
- Diagnosing boot failures and systemd service issues
- Investigating permission denied errors
- Understanding VM initialization problems
- Debugging network and device configuration

## Virtiofsd Logs

The virtiofsd daemon logs are written to `/run/virtiofsd.log` and `/run/virtiofsd-<mount-name>.log` for each filesystem mount. These logs show filesystem sharing operations between the container and VM.

To view virtiofsd logs:

    # Main virtiofsd log
    podman exec <container-id> cat /run/virtiofsd.log

    # Logs for additional bind mounts
    podman exec <container-id> cat /run/virtiofsd-workspace.log

Virtiofsd logs are helpful for:
- Debugging filesystem access issues
- Understanding file handle support warnings
- Investigating mount-related errors

# SEE ALSO

**bcvk**(8)

# VERSION

<!-- VERSION PLACEHOLDER -->
