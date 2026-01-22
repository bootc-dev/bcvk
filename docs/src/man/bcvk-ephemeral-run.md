# NAME

bcvk-ephemeral-run - Run bootc containers as stateless VMs

# SYNOPSIS

**bcvk ephemeral run** \[*OPTIONS*\] *IMAGE*

# DESCRIPTION

Run bootc containers as stateless VMs managed by podman. The VM boots directly
from the container image's filesystem via virtiofs, with no disk image creation
required. This makes startup fast and the VM stateless by default.

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
- **Isolation**: The VM runs in a contained environment separate from the host
- **Fast I/O**: virtiofs provides efficient filesystem access between container and VM
- **Resource Efficiency**: Leverages existing container infrastructure while providing full VM capabilities

## Container-VM Relationship

The relationship between the podman container and the VM inside it:

- **Podman Container**: Acts as the virtualization environment, providing QEMU and system services
- **QEMU Process**: Runs inside the podman container, creating the actual virtual machine
- **VM Guest**: The bootc container image runs as a complete operating system inside the VM
- **Filesystem Sharing**: The container's root filesystem is shared with the VM via virtiofs at runtime

This design allows bcvk to provide VM-like isolation and boot behavior while leveraging container tooling.

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

**--debug-entrypoint**=*DEBUG_ENTRYPOINT*

    Do not run the default entrypoint directly, but instead invoke the provided command (e.g. `bash`)

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

<!-- END GENERATED OPTIONS -->

# EXAMPLES

## Build and Test Workflow

The most common use case is testing container images you're building:

    # Build your bootc container image
    podman build -t localhost/mybootc .

    # Run it as an ephemeral VM (background, auto-cleanup, SSH keys)
    bcvk ephemeral run -d --rm -K --name test localhost/mybootc

    # SSH in to verify it works
    bcvk ephemeral ssh test

    # Stop when done (container auto-removed due to --rm)
    podman stop test

For a faster iteration loop, use **bcvk-ephemeral-run-ssh**(8) which combines
run and SSH into one command with automatic cleanup.

## Common Flag Combinations

**Testing a public image**:

    bcvk ephemeral run -d --rm -K --name testvm quay.io/fedora/fedora-bootc:42
    bcvk ephemeral ssh testvm

**Development with mounted source code**:

    bcvk ephemeral run -d --rm -K \
        --bind /home/user/project:src \
        --name devvm localhost/mybootc
    bcvk ephemeral ssh devvm
    # Files available at /run/virtiofs-mnt-src inside VM

**Resource-intensive workloads**:

    bcvk ephemeral run -d --rm -K \
        --memory 8G --vcpus 4 \
        --name bigvm localhost/mybootc

**Debugging boot issues**:

    bcvk ephemeral run --console --name debugvm localhost/mybootc

## Understanding the Flags

**-d, --detach**: Run in background. Without this, the container runs in
foreground and you see QEMU output directly.

**--rm**: Auto-remove the container when it stops. Highly recommended for
ephemeral testing to avoid accumulating stopped containers.

**-K, --ssh-keygen**: Generate SSH keypair and inject into the VM via
systemd credentials. Required for **bcvk ephemeral ssh** to work.

**--name**: Assign a name for easy reference with other commands like
**bcvk ephemeral ssh**.

## Bind Mounts

Share host directories with the VM using **--bind** or **--ro-bind**:

    # Read-write mount
    bcvk ephemeral run -d --rm -K --bind /host/path:name --name vm image

    # Inside VM, access at:
    /run/virtiofs-mnt-name

    # Read-only mount (safer for sensitive data)
    bcvk ephemeral run -d --rm -K --ro-bind /etc/myconfig:config --name vm image

## Disk File Mounts

Mount raw disk images as block devices:

    # Create a test disk
    truncate -s 10G /tmp/testdisk.raw

    # Mount into VM
    bcvk ephemeral run -d --rm -K \
        --mount-disk-file /tmp/testdisk.raw:testdisk \
        --name vm localhost/mybootc

    # Inside VM: /dev/disk/by-id/virtio-testdisk

## Port Forwarding

For SSH port forwarding (recommended), use **bcvk ephemeral ssh** with **-L** or **-R**:

    # Forward VM port 80 to localhost:8080
    bcvk ephemeral ssh myvm -L 8080:localhost:80

For network-level port forwarding via podman, configure slirp4netns:

    bcvk ephemeral run -d --rm -K \
        --network slirp4netns:port_handler=slirp4netns,allow_host_loopback=true \
        --name webvm localhost/mybootc

## Instance Types

Use predefined instance types for consistent resource allocation:

    bcvk ephemeral run -d --rm -K --itype u1.small --name vm localhost/mybootc
    bcvk ephemeral run -d --rm -K --itype u1.medium --name vm localhost/mybootc
    bcvk ephemeral run -d --rm -K --itype u1.large --name vm localhost/mybootc

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
