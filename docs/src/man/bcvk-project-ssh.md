# NAME

bcvk-project-ssh - SSH into the project VM

# SYNOPSIS

**bcvk project ssh** [*OPTIONS*]

# DESCRIPTION

SSH into the project VM.

Automatically starts the VM if it's stopped.

## Manual upgrade trigger

The `--update` (or `-A`) flag performs a two-stage bootc upgrade before establishing
the SSH connection:

1. First, it runs `bootc upgrade` to fetch and stage the update. Any errors during
   this phase are caught and reported before the VM reboots.

2. If staging succeeds, it runs `bootc upgrade --apply` to apply the update and
   reboot the VM. The command waits for the VM to come back online after reboot.

This allows you to manually trigger an immediate upgrade of the bootc deployment
in your VM, useful when you've built a new version of your container image and
want to deploy it right away without waiting for automatic updates.

The upgrade command has a 10-minute timeout and streams its output to the
console in real-time, so you can monitor the upgrade progress. After the
VM reboots and comes back online, the SSH connection is automatically established
(either opening an interactive shell or running the specified command).

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**COMMAND**

    Command to execute in the VM (if empty, opens interactive shell)

**--connect**=*CONNECT*

    Libvirt connection URI (defaults to qemu:///session)

**-A**, **--update**

    Run bootc upgrade in two stages (fetch/stage, then apply/reboot) before connecting

<!-- END GENERATED OPTIONS -->

# EXAMPLES

Open an interactive SSH session:

    bcvk project ssh

If the VM is stopped, it will be started automatically before connecting.

Run a single command in the VM:

    bcvk project ssh ls -la /workspace

Execute a command with multiple arguments:

    bcvk project ssh -- systemctl status myservice

The `--` separator ensures all following arguments are passed to the VM command.

Trigger a bootc upgrade before connecting:

    bcvk project ssh --update

Or using the short flag:

    bcvk project ssh -A

This runs `bootc upgrade --apply` and then opens an interactive shell after
the upgrade completes.

Trigger upgrade and run a command:

    bcvk project ssh --update bootc status

This upgrades the deployment and then immediately checks the bootc status to
verify the new deployment.

Example workflow for manual upgrade:

    # Rebuild your container image
    podman build -t localhost/my-app:dev .

    # Immediately apply the update and reconnect
    bcvk project ssh -A

    # Verify the deployment
    bootc status

You can also check the upgrade logs:

    bcvk project ssh -A journalctl -u bootc-fetch-apply-updates -n 50

# SEE ALSO

**bcvk**(8), **bcvk-project**(8), **bcvk-project-up**(8), **bcvk-project-down**(8)

# VERSION

<!-- VERSION PLACEHOLDER -->
