# NAME

bcvk-libvirt-run - Run a bootable container as a persistent VM

# SYNOPSIS

**bcvk libvirt run** [*OPTIONS*]

# DESCRIPTION

Run a bootable container as a persistent VM

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**IMAGE**

    Container image to run as a bootable VM

    This argument is required.

**--name**=*NAME*

    Name for the VM (auto-generated if not specified)

**--memory**=*MEMORY*

    Memory size (e.g. 4G, 2048M, or plain number for MB)

    Default: 4G

**--cpus**=*CPUS*

    Number of virtual CPUs for the VM

    Default: 2

**--disk-size**=*DISK_SIZE*

    Disk size for the VM (e.g. 20G, 10240M, or plain number for bytes)

    Default: 20G

**--filesystem**=*FILESYSTEM*

    Root filesystem type for installation

    Default: ext4

**-p**, **--port**=*PORT_MAPPINGS*

    Port mapping from host to VM

**-v**, **--volume**=*VOLUMES*

    Volume mount from host to VM

**--network**=*NETWORK*

    Network mode for the VM

    Default: user

**--detach**

    Keep the VM running in background after creation

**--ssh**

    Automatically SSH into the VM after creation

**--bind-storage-ro**

    Mount host container storage (RO) at /run/virtiofs-mnt-hoststorage

<!-- END GENERATED OPTIONS -->

# EXAMPLES

Create and start a persistent VM:

    bcvk libvirt run --name my-server quay.io/fedora/fedora-bootc:42

Create a VM with custom resources:

    bcvk libvirt run --name webserver --memory 8192 --cpus 8 --disk-size 50G quay.io/centos-bootc/centos-bootc:stream10

Create a VM with port forwarding:

    bcvk libvirt run --name webserver --port 8080:80 quay.io/centos-bootc/centos-bootc:stream10

Create a VM with volume mount:

    bcvk libvirt run --name devvm --volume /home/user/code:/workspace quay.io/fedora/fedora-bootc:42

Create a VM and automatically SSH into it:

    bcvk libvirt run --name testvm --ssh quay.io/fedora/fedora-bootc:42

Create a VM with access to host container storage for bootc upgrade:

    bcvk libvirt run --name upgrade-test --bind-storage-ro quay.io/fedora/fedora-bootc:42

Server management workflow:

    # Create a persistent server VM
    bcvk libvirt run --name production-server --memory 8192 --cpus 4 --disk-size 100G my-server-image
    
    # Check status
    bcvk libvirt list
    
    # Access for maintenance
    bcvk libvirt ssh production-server

# SEE ALSO

**bcvk**(8)

# VERSION

v0.1.0
