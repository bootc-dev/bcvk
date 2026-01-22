# NAME

bcvk-ephemeral-ssh - Connect to running VMs via SSH

# SYNOPSIS

**bcvk ephemeral ssh** *CONTAINER_NAME* \[*SSH_ARGS*\]

# DESCRIPTION

Connect to a running ephemeral VM via SSH. This command locates the VM's
SSH port and connects using the SSH key that was injected when the VM
was started with **-K** or **--ssh-keygen**.

## Prerequisites

The target VM must have been started with SSH key injection:

    bcvk ephemeral run -d --rm -K --name myvm image

Without **-K**, the VM will not have SSH keys configured and this command
will fail to authenticate.

## How It Works

1. Queries the podman container to find the VM's SSH port
2. Retrieves the SSH private key from the container
3. Connects using the dynamically assigned port and key
4. Passes any additional arguments to the ssh client

## VM Lifecycle

SSH connections do not affect the VM lifecycle for background VMs:

- **Background VMs** (started with **-d**): Continue running after SSH disconnect.
  You can reconnect as many times as needed.
- **Auto-cleanup VMs** (started with **--rm**): Container is removed when the VM
  stops, but SSH disconnect alone does not stop the VM.

To stop a background VM, use **podman stop**.

For automatic cleanup on SSH exit, use **bcvk-ephemeral-run-ssh**(8) instead.

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**CONTAINER_NAME**

    Name or ID of the container running the target VM

    This argument is required.

**ARGS**

    SSH arguments like -v, -L, -o

<!-- END GENERATED OPTIONS -->

# EXAMPLES

## Basic Connection

Connect to a running VM:

    bcvk ephemeral ssh myvm

## Running Remote Commands

Execute a command directly:

    bcvk ephemeral ssh myvm 'systemctl status'

Check disk usage:

    bcvk ephemeral ssh myvm 'df -h'

View journal logs:

    bcvk ephemeral ssh myvm 'journalctl -f'

## SSH Options

Enable verbose output for debugging connection issues:

    bcvk ephemeral ssh myvm -v

Forward a local port to the VM:

    bcvk ephemeral ssh myvm -L 8080:localhost:80
    # Access VM's port 80 at localhost:8080

Reverse port forwarding (expose host port to VM):

    bcvk ephemeral ssh myvm -R 3000:localhost:3000

## Typical Workflow

    # Start a VM in the background
    bcvk ephemeral run -d --rm -K --name testvm quay.io/fedora/fedora-bootc:42

    # Connect and do some work
    bcvk ephemeral ssh testvm
    # ... interactive session ...
    # exit

    # Reconnect later
    bcvk ephemeral ssh testvm

    # Run a quick command without interactive session
    bcvk ephemeral ssh testvm 'cat /etc/os-release'

    # Stop the VM when done
    podman stop testvm

## File Transfer

Use scp-style operations by getting the SSH details:

    # For now, use podman exec for file transfer
    podman cp localfile testvm:/path/in/container

    # Or mount a shared directory when starting the VM
    bcvk ephemeral run -d --rm -K --bind /host/path:shared --name vm image

# TROUBLESHOOTING

**Connection refused**: The VM may still be booting. Wait a few seconds and retry.

**Permission denied**: Ensure the VM was started with **-K** for SSH key injection.

**Host key verification failed**: Each VM gets a unique host key. Use **-o StrictHostKeyChecking=no** if needed.

# SEE ALSO

**bcvk**(8), **bcvk-ephemeral**(8), **bcvk-ephemeral-run**(8),
**bcvk-ephemeral-run-ssh**(8), **ssh**(1)

# VERSION

<!-- VERSION PLACEHOLDER -->
