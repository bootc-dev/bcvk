# NAME

bcvk-libvirt-list - List available bootc volumes with metadata

# SYNOPSIS

**bcvk libvirt list** [*DOMAIN_NAME*] [*OPTIONS*]

# DESCRIPTION

List available bootc domains with metadata. When a domain name is provided, returns information about that specific domain only.

When using `--format=json` with a specific domain name, the output is a single JSON object (not an array), making it easy to extract SSH credentials and connection information using tools like `jq`.

# OPTIONS

**DOMAIN_NAME**

    Optional domain name to query. When specified, returns information about only this domain.

# OPTIONS

<!-- BEGIN GENERATED OPTIONS -->
**DOMAIN_NAME**

    Domain name to query (returns only this domain)

**--format**=*FORMAT*

    Output format

    Possible values:
    - table
    - json
    - yaml

    Default: table

**-a**, **--all**

    Show all domains including stopped ones

**--label**=*LABEL*

    Filter domains by label

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

Query a specific domain:

    bcvk libvirt list my-domain

## Working with SSH credentials via JSON output

Connect via SSH using extracted credentials:

    # Query once, save to file, then extract credentials
    DOMAIN_NAME="mydomain"

    # Query domain info once and save to file
    bcvk libvirt list $DOMAIN_NAME --format=json > /tmp/domain-info.json

    # Extract SSH private key
    jq -r '.ssh_private_key' /tmp/domain-info.json > /tmp/key.pem
    chmod 600 /tmp/key.pem

    # Extract SSH port
    SSH_PORT=$(jq -r '.ssh_port' /tmp/domain-info.json)

    # Connect via SSH
    ssh -o IdentitiesOnly=yes -i /tmp/key.pem -p $SSH_PORT -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null root@127.0.0.1

    # Cleanup
    rm /tmp/domain-info.json /tmp/key.pem

This is useful for automation scripts or when you need direct SSH access without using `bcvk libvirt ssh`.

# SEE ALSO

**bcvk**(8)

# VERSION

v0.1.0
