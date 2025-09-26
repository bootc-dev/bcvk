# NAME

bcvk-libvirt-list - List available bootc volumes with metadata

# SYNOPSIS

**bcvk libvirt list** [*OPTIONS*]

# DESCRIPTION

List available bootc volumes with metadata

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**--format**=*FORMAT*

    Output format

    Default: table

**-a**, **--all**

    Show all domains including stopped ones

<!-- END GENERATED OPTIONS -->

# EXAMPLES

List all running bootc VMs:

    bcvk libvirt list

List all bootc VMs including stopped ones:

    bcvk libvirt list --all

Show VM status in your workflow:

    # Check what VMs are running
    bcvk libvirt list
    
    # Start a specific VM if needed
    bcvk libvirt start my-server

# SEE ALSO

**bcvk**(8)

# VERSION

v0.1.0
