# NAME

bcvk-project-up - Create or start the project VM

# SYNOPSIS

**bcvk project up** [*OPTIONS*]

# DESCRIPTION

Create or start the project VM.

Automatically names and manages a VM scoped to the current project directory.
Won't recreate if a VM with the same name already exists.

## Lifecycle binding to parent process

The expectation is that this tool is invoked from a persistent interactive
shell when working on a project. By default, a "lifecycle-bind" child process
will be run in the background which monitors the parent, and when it exits
then the VM will be shut down. This provides convenient semantics for users
of IDEs and similar tools. Use `--no-lifecycle-bind` to disable.

## Automatic updates

The `--auto-update` flag enables rapid iteration during development by
configuring the VM to automatically check for and apply bootc updates every
30 seconds. This is accomplished by injecting systemd unit dropins that:

- Configure `bootc-fetch-apply-updates.service` to use the host's container
  storage via virtiofs (mounted at `/run/host-container-storage`)
- Override `bootc-fetch-apply-updates.timer` to run every 30 seconds instead
  of the default interval

This allows you to build a new version of your container image on the host,
and have it automatically deployed to the VM within 30 seconds - perfect for
fast development iteration cycles.

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**--connect**=*CONNECT*

    Libvirt connection URI (defaults to qemu:///session)

**--ssh**

    Automatically SSH into the VM after creation

**-L**, **--no-lifecycle-bind**

    Disable lifecycle binding (don't shutdown VM when parent exits)

**--auto-update**

    Enable automatic updates via bootc-fetch-apply-updates every 30s

**-R**, **--reset**

    Reset: remove existing VM (force stop and delete) before creating new one

<!-- END GENERATED OPTIONS -->

# EXAMPLES

Start a project VM from existing configuration:

    bcvk project up

This requires a `.bcvk/config.toml` file in the current directory. If you don't have one yet,
run `bcvk project init` to create it interactively.

Minimum required configuration:

    [vm]
    image = "quay.io/fedora/fedora-bootc:42"

The project directory is automatically mounted at `/run/src` in the VM as read-only.

Start and immediately SSH into the VM:

    bcvk project up --ssh

Disable automatic lifecycle binding:

    bcvk project up --no-lifecycle-bind

This keeps the VM running even after the parent process exits.

Start with automatic updates enabled:

    bcvk project up --auto-update

This enables automatic bootc updates every 30 seconds, ideal for development
workflows where you're frequently rebuilding your container image.

Example development workflow with auto-update:

    # Start VM with auto-update enabled
    bcvk project up --auto-update --ssh

    # In another terminal, rebuild your image
    podman build -t localhost/my-app:dev .

    # The VM will detect and apply the update within 30 seconds
    # You can watch it happen:
    journalctl -f -u bootc-fetch-apply-updates

# SEE ALSO

**bcvk**(8), **bcvk-project**(8), **bcvk-project-down**(8), **bcvk-project-ssh**(8)

# VERSION

<!-- VERSION PLACEHOLDER -->
