# NAME

bcvk-project - Project-scoped VM management
# SYNOPSIS

**bcvk project** [*OPTIONS*]

**bcvk p** [*OPTIONS*]

# DESCRIPTION

Project-scoped VM management. A "project" is typically
a git repository, defining a bootc-based system.

Often one might use this tool as a way to conveniently
test an operating system (variant) locally before deployment.

## Similarity to Vagrant

Similar to Vagrant, `bcvk project` manages development VMs on a per-directory
basis. The key differences are that bcvk uses bootc container images instead
of traditional VM images, and all configuration is stored in `.bcvk/config.toml`
rather than a Vagrantfile.

## Development workflow

The project commands are designed for rapid iteration when developing bootc-based
systems:

1. **Initial setup**: `bcvk project init` creates a `.bcvk/config.toml` configuration
2. **Start VM**: `bcvk project up` creates and starts the VM
3. **Make changes**: Edit your Containerfile and rebuild the image
4. **Test updates**: Either wait for automatic updates (with `--auto-update`) or
   manually trigger with `bcvk project ssh -A`

For development workflows, consider using `bcvk project up --auto-update` to
enable automatic deployment of changes every 30 seconds, or use `bcvk project ssh -A`
to manually trigger immediate upgrades when you've rebuilt your image.

<!-- BEGIN GENERATED OPTIONS -->
<!-- END GENERATED OPTIONS -->

# EXAMPLES

Initialize a new project with the interactive wizard:

    cd /path/to/my-project
    bcvk project init

Start an existing project VM:

    bcvk project up

SSH into the project VM:

    bcvk project ssh

Shut down the project VM:

    bcvk project down

Remove the project VM entirely:

    bcvk project rm

Complete development workflow with automatic updates:

    # Initialize project
    bcvk project init

    # Start VM with auto-update enabled
    bcvk project up --auto-update --ssh

    # In another terminal, make changes and rebuild
    vim Containerfile
    podman build -t localhost/my-app:dev .

    # Changes are automatically applied within 30 seconds
    # Or manually trigger immediate upgrade:
    bcvk project ssh -A

# SEE ALSO

**bcvk**(8), **bcvk-project-init**(8), **bcvk-project-up**(8), **bcvk-project-down**(8), **bcvk-project-ssh**(8), **bcvk-project-rm**(8)

# VERSION

<!-- VERSION PLACEHOLDER -->
