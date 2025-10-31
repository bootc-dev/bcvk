# NAME

bcvk-project-init - Initialize project configuration interactively

# SYNOPSIS

**bcvk project init** [*OPTIONS*]

# DESCRIPTION

Initialize project configuration interactively by creating a `.bcvk/config.toml` file
in the current directory. The wizard will guide you through selecting a bootc container
image and optionally setting a custom project name.

The configuration file is minimal by default, containing only the required `vm.image`
field and any custom settings you specify. Additional configuration such as memory,
CPU, disk size, and volume mounts can be added manually to the generated file.

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**-f**, **--force**

    Overwrite existing configuration if it exists

<!-- END GENERATED OPTIONS -->

# EXAMPLES

Initialize a new project in the current directory:

    cd /path/to/my-project
    bcvk project init

The wizard will prompt you to select from available bootc images or enter a custom image name.

Overwrite existing configuration:

    bcvk project init --force

This is useful if you want to reset your project configuration or if the config file is corrupted.

Example generated configuration:

    [vm]
    image = "quay.io/fedora/fedora-bootc:42"

Or with a custom project name:

    [project]
    name = "my-custom-name"

    [vm]
    image = "quay.io/centos-bootc/centos-bootc:stream10"

# SEE ALSO

**bcvk**(8), **bcvk-project**(8), **bcvk-project-up**(8)

# VERSION

<!-- VERSION PLACEHOLDER -->
