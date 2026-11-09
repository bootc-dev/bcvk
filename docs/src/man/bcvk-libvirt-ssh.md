# NAME

bcvk-libvirt-ssh - SSH to libvirt domain with embedded SSH key

# SYNOPSIS

**bcvk libvirt ssh** [*OPTIONS*]

# DESCRIPTION

SSH to libvirt domain with embedded SSH key

If the domain is shut off, it is started first, and the connection is
made once SSH is reachable. Domains in other states (e.g. paused) are
not changed and result in an error.

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**DOMAIN_NAME**

    Name of the libvirt domain to connect to

    This argument is required.

**COMMAND**

    Command to execute on remote host

**--user**=*USER*

    SSH username to use for connection (defaults to 'root')

    Default: root

**--strict-host-keys**

    Use strict host key checking

**--timeout**=*TIMEOUT*

    SSH connection timeout in seconds

    Default: 5

**--log-level**=*LOG_LEVEL*

    SSH log level

    Default: ERROR

**--extra-options**=*EXTRA_OPTIONS*

    Extra SSH options in key=value format

<!-- END GENERATED OPTIONS -->

# EXAMPLES

SSH into a running libvirt VM:

    bcvk libvirt ssh my-server

SSH into a stopped VM, starting it first:

    bcvk libvirt stop my-server
    bcvk libvirt ssh my-server

Execute a command on the VM:

    bcvk libvirt ssh my-server 'systemctl status'

SSH with a specific user:

    bcvk libvirt ssh --user admin my-server

Connect to a VM with extended timeout:

    bcvk libvirt ssh --timeout 60 my-server

# SEE ALSO

**bcvk**(8)

# VERSION

v0.1.0
