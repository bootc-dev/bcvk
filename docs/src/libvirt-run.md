# Libvirt Integration Concepts

Libvirt integration provides persistent, production-ready virtual machine management for bootc containers. This guide covers the concepts, workflows, and architectural patterns for using bcvk with libvirt.

## What is Libvirt Integration?

Libvirt integration transforms bootc containers into persistent virtual machines managed by the libvirt virtualization API. Unlike ephemeral VMs, these are full-featured virtual machines suitable for production workloads.

### Key Characteristics

**Persistent Lifecycle**: VMs continue to exist between host reboots
**Professional Management**: Full libvirt API and tooling support
**Production Ready**: Enterprise-grade virtualization features
**Hardware Integration**: Direct access to host hardware resources

### Libvirt vs Ephemeral VMs

**Libvirt VMs**:
- Persistent storage and configuration
- Survive host reboots
- Full VM lifecycle management
- Enterprise features (snapshots, migration, monitoring)
- Production workload suitability

**Ephemeral VMs**:
- Temporary, testing-focused
- Automatic cleanup
- Development and debugging
- Quick iteration cycles

## Integration Architecture

### How bcvk Works with Libvirt

1. **Container Analysis**: bcvk analyzes the container image
2. **Disk Image Creation**: Converts container to VM disk image
3. **VM Definition**: Creates libvirt domain XML configuration
4. **Resource Allocation**: Configures CPU, memory, and storage
5. **Network Setup**: Establishes network connectivity
6. **VM Registration**: Registers VM with libvirt daemon

### Storage Integration

**Storage Pools**: Libvirt storage pools manage VM disk images
**Volume Management**: Automatic volume creation and lifecycle
**Format Support**: QCOW2, raw, and other disk formats
**Snapshot Capability**: Built-in snapshot and backup features

### Network Integration

**Virtual Networks**: Integration with libvirt network management
**Bridge Networking**: Direct host network integration
**Isolated Networks**: Secure network segmentation
**NAT and Routing**: Flexible network topologies

## When to Use Libvirt Integration

### Production Workloads

Libvirt integration is ideal for:
- **Server applications**: Long-running services and daemons
- **Database systems**: Persistent data storage requirements
- **Web services**: Production web applications and APIs
- **Enterprise applications**: Business-critical workloads

### Development Infrastructure

Use libvirt VMs for:
- **Persistent development environments**: Long-term development setups
- **Shared development resources**: Multi-user development platforms
- **Integration testing**: Stable test environments
- **Staging environments**: Production-like testing platforms

### Infrastructure Services

Perfect for:
- **CI/CD runners**: Build and deployment systems
- **Monitoring infrastructure**: Metrics and logging systems
- **Network services**: DNS, DHCP, and other network services
- **Security services**: Firewalls, VPNs, and security appliances

## Libvirt Workflow Patterns

### Basic VM Creation

```bash
# Create a simple persistent VM
bcvk libvirt run quay.io/myapp/server:latest
```

**Benefits**:
- Automatic VM configuration
- Persistent storage
- Libvirt management integration
- Default resource allocation

### Production Service Deployment

```bash
# Deploy production service with specific resources
bcvk libvirt run \
  --name production-api \
  --memory 8192 \
  --cpus 4 \
  --autostart \
  quay.io/myapp/api:v1.0
```

**Benefits**:
- Named VM for identification
- Resource allocation control
- Automatic startup configuration
- Production-grade setup

### Development Environment Setup

```bash
# Create development environment with host integration
bcvk libvirt run \
  --name dev-environment \
  --memory 4096 \
  --cpus 2 \
  --network default \
  --ssh \
  quay.io/myapp/dev:latest
```

**Benefits**:
- Network connectivity
- SSH access for development
- Persistent development state
- Host integration capabilities

### Host Directory Mounting

```bash
# Create VM with host directory mounted into the guest
bcvk libvirt run \
  --name dev-environment \
  --volume /home/chris/projects/foo:src \
  --volume /home/chris/data:data \
  --ssh \
  quay.io/myapp/dev:latest
```

The `--volume` flag allows you to share directories from the host into the VM using virtiofs.
The format is `--volume HOST_PATH:TAG`, where:
- `HOST_PATH` must be an existing directory on the host
- `TAG` is a virtiofs tag name you choose (used to mount the filesystem in the guest)

After the VM is created, you'll need to mount the virtiofs filesystems in the guest. For example:

```bash
# In the guest VM
mkdir -p /mnt/src
mount -t virtiofs src /mnt/src

mkdir -p /mnt/data
mount -t virtiofs data /mnt/data
```

You can mount each virtiofs filesystem anywhere you like in the guest using its tag name.
The bcvk command output will show you the tag names for each volume.

**Benefits**:
- Live file sharing between host and guest
- No copying or synchronization needed
- Near-native filesystem performance
- Read-write access (use with caution)

### Container Storage Integration

```bash
# Create VM with access to host container storage for bootc upgrades
bcvk libvirt run \
  --name upgrade-test \
  --bind-storage-ro \
  --ssh \
  quay.io/fedora/fedora-bootc:42
```

With this, a virtiofs mount named `hoststorage` is provisioned. There isn't
yet automatic mounting, but you can inject code to do so that performs
`mkdir -p /run/virtiofs-mnt-hoststorage && mount -t virtiofs hoststorage /run/virtiofs-mnt-hoststorage`.

Then on your host system after you've done a `podman build` that results in a new image `localhost/bootc`,
in the guest system you can point bootc to use it via e.g.
```
env STORAGE_OPTS=additionalimagestore=/run/virtiofs-mnt-hoststorage bootc switch --transport containers-storage localhost/bootc
```

You currently need to add the `STORAGE_OPTS` each time you invoke `bootc` - but there after e.g.
```
env STORAGE_OPTS=additionalimagestore=/run/virtiofs-mnt-hoststorage bootc upgrade
```

will work.

## Secure Boot Configuration

UEFI Secure Boot ensures that only signed and trusted software can run during the boot process. bcvk supports loading existing Secure Boot keys for libvirt VMs.

### Prerequisites

- `edk2-ovmf` package (provides UEFI firmware files)
- `python3-virt-firmware` package (provides `virt-fw-vars` tool)
- `openssl` (for certificate generation)

Install on Fedora/RHEL:
```bash
sudo dnf install -y edk2-ovmf python3-virt-firmware openssl
```

### Using Secure Boot

Enable Secure Boot with existing keys:

```bash
bcvk libvirt run --firmware uefi-secure --secure-boot-keys /path/to/keys quay.io/myimage:latest
```

This will load existing PK, KEK, and db certificates from the specified directory, create a custom OVMF_VARS template with enrolled keys, and configure the VM to use Secure Boot firmware.

### Generating Keys

Secure Boot keys must be generated externally:

```bash
# Generate keys in a directory
mkdir -p ./my-secure-boot-keys
cd ./my-secure-boot-keys

# Generate Platform Key (PK)
openssl req -newkey rsa:4096 -nodes -keyout PK.key -new -x509 -sha256 -days 3650 -subj '/CN=Platform Key/' -out PK.crt

# Generate Key Exchange Key (KEK)
openssl req -newkey rsa:4096 -nodes -keyout KEK.key -new -x509 -sha256 -days 3650 -subj '/CN=Key Exchange Key/' -out KEK.crt

# Generate Signature Database key (db)
openssl req -newkey rsa:4096 -nodes -keyout db.key -new -x509 -sha256 -days 3650 -subj '/CN=Signature Database key/' -out db.crt

# Generate GUID
uuidgen > GUID.txt
```

The Secure Boot configuration requires:

- **PK (Platform Key)**: Root of trust, controls KEK updates (PK.crt)
- **KEK (Key Exchange Key)**: Authorizes db/dbx updates (KEK.crt)
- **db (Signature Database)**: Contains authorized signatures (db.crt)
- **GUID**: Unique identifier for the key owner (GUID.txt)

### Firmware Options

bcvk provides three firmware options:

- `--firmware uefi-secure` (default): UEFI with Secure Boot enabled
- `--firmware uefi-insecure`: UEFI with Secure Boot explicitly disabled
- `--firmware bios`: Legacy BIOS mode

### Troubleshooting Secure Boot

**Missing virt-fw-vars**:
```bash
sudo dnf install -y python3-virt-firmware
```

**Missing OVMF files**:
```bash
sudo dnf install -y edk2-ovmf
```

**Missing key files**: Generate keys first using the commands above.

**Permission issues**: Ensure read permissions to the key directory:
```bash
chmod -R 644 /path/to/your/keys
chmod 755 /path/to/your/keys
```

**Verify Secure Boot status in VM**:
```bash
mokutil --sb-state  # Should show "SecureBoot enabled"
```

## Resource Management Concepts

### CPU Allocation

**vCPU Assignment**:
- Virtual CPUs mapped to physical cores
- CPU topology configuration options
- Performance vs density trade-offs
- NUMA topology considerations

**CPU Features**:
- Host CPU feature passthrough
- CPU model specification
- Performance optimization settings
- Security feature configuration

### Memory Management

**Memory Allocation**:
- Static vs dynamic memory assignment
- Memory ballooning for efficiency
- NUMA memory placement
- Huge page support for performance

**Memory Security**:
- Memory isolation between VMs
- Memory encryption capabilities
- Secure memory allocation
- Memory pressure handling

### Storage Architecture

**Disk Image Management**:
- Persistent disk image storage
- Copy-on-write disk efficiency
- Disk image format selection
- Storage pool organization

**Performance Considerations**:
- Storage backend selection (file, LVM, Ceph)
- Disk cache configuration
- I/O thread allocation
- Storage network optimization

## Network Architecture Concepts

### Virtual Networking

**Network Models**:
- **NAT networks**: Outbound connectivity with isolation
- **Bridge networks**: Direct host network integration
- **Isolated networks**: Complete network isolation
- **Routed networks**: Advanced routing scenarios

**Network Services**:
- DHCP for automatic IP configuration
- DNS for name resolution
- Firewall integration for security
- Quality of Service (QoS) controls

### Integration Patterns

**Host Network Integration**:
- Bridge to physical network interfaces
- VLAN tagging and trunk interfaces
- Bond interface support
- Network namespace integration

**Service Discovery**:
- DNS-based service discovery
- Network policy implementation
- Load balancing integration
- Service mesh connectivity

## Security and Isolation

### Virtualization Security

**Hardware-Level Isolation**:
- CPU virtualization extensions (VT-x, AMD-V)
- Memory management unit isolation
- I/O device virtualization
- Interrupt handling separation

**Software Security Features**:
- SELinux/AppArmor integration
- Secure Boot support
- TPM (Trusted Platform Module) integration
- UEFI firmware security

### Network Security

**Traffic Isolation**:
- Virtual network segmentation
- Firewall rule enforcement
- Network access control lists
- Traffic filtering and inspection

**Security Policies**:
- Mandatory access controls
- Network security policies
- Audit and compliance logging
- Intrusion detection integration

## High Availability and Scalability

### VM Lifecycle Management

**Availability Features**:
- Automatic VM restart on failure
- Host failure detection and recovery
- VM migration for maintenance
- Resource monitoring and alerting

**Backup and Recovery**:
- VM snapshot capabilities
- Incremental backup support
- Point-in-time recovery
- Disaster recovery procedures

### Clustering and Scale

**Multi-Host Deployment**:
- VM distribution across hosts
- Load balancing and traffic distribution
- Shared storage integration
- Cluster management integration

**Resource Scaling**:
- Dynamic resource adjustment
- Vertical scaling (CPU/memory)
- Horizontal scaling (multiple VMs)
- Auto-scaling integration

## Integration with Infrastructure Tools

### Configuration Management

**Ansible Integration**:
- VM provisioning automation
- Configuration management
- Application deployment
- Operational task automation

**Terraform Support**:
- Infrastructure as code
- Declarative VM management
- Resource dependency management
- Multi-cloud deployment patterns

### Monitoring and Observability

**Performance Monitoring**:
- VM resource utilization tracking
- Performance metrics collection
- Capacity planning data
- Trend analysis and alerting

**Health Monitoring**:
- VM health check automation
- Service availability monitoring
- Log aggregation and analysis
- Incident response integration

## Comparison with Other Virtualization

### vs Cloud Instances

**Libvirt VMs provide**:
- **Local control**: Complete infrastructure control
- **Cost efficiency**: No per-hour cloud charges
- **Performance**: Direct hardware access
- **Privacy**: Data stays on-premises

### vs Container Orchestration

**Libvirt advantages**:
- **Stronger isolation**: Hardware-level separation
- **Legacy compatibility**: Support for non-containerized workloads
- **Resource allocation**: Guaranteed resource allocation
- **Boot process control**: Full OS boot and system services

### vs Bare Metal

**Virtualization benefits**:
- **Resource efficiency**: Multiple workloads per host
- **Flexibility**: Easy resource reallocation
- **Isolation**: Workload separation and security
- **Management**: Centralized VM management

## Performance Optimization

### Host System Optimization

**Hardware Configuration**:
- CPU virtualization feature enablement
- Memory allocation and NUMA configuration
- Storage subsystem optimization
- Network interface configuration

**Host OS Tuning**:
- Kernel parameter optimization
- Scheduler configuration
- Memory management tuning
- I/O scheduler optimization

### VM-Level Optimization

**VM Configuration**:
- CPU model and feature selection
- Memory balloon driver configuration
- Disk cache and I/O settings
- Network driver optimization

**Application Integration**:
- Guest OS optimization
- Application-specific tuning
- Service configuration optimization
- Monitoring and profiling integration

## Best Practices

### Production Deployment

- **Resource planning**: Right-size VMs for workloads
- **Security hardening**: Implement security best practices
- **Monitoring setup**: Establish comprehensive monitoring
- **Backup strategy**: Implement reliable backup procedures

### Development Workflow

- **Environment consistency**: Maintain development/production parity
- **Version control**: Track VM configurations and changes
- **Testing automation**: Automate VM testing procedures
- **Documentation**: Maintain clear operational documentation

### Operational Excellence

- **Change management**: Control and audit configuration changes
- **Incident response**: Prepare for operational issues
- **Capacity planning**: Monitor and plan for growth
- **Security management**: Regular security updates and assessments

## Troubleshooting Concepts

### Common Issues

**VM Creation Problems**:
- Insufficient host resources
- Storage pool configuration issues
- Network configuration problems
- Image compatibility issues

**Performance Issues**:
- Resource contention
- Storage bottlenecks
- Network latency
- Host system overload

**Connectivity Problems**:
- Network configuration errors
- Firewall blocking
- DNS resolution issues
- Service configuration problems

### Debugging Approaches

- **Systematic diagnosis**: Check components systematically
- **Log analysis**: Examine libvirt and system logs
- **Resource monitoring**: Track resource utilization
- **Network testing**: Validate network connectivity

## Next Steps

For detailed command syntax and options, see:
- [bcvk-libvirt-run(8)](./man/bcvk-libvirt-run.md) - Complete command reference
- [VM Lifecycle Management](./libvirt-manage.md) - Managing libvirt VMs
- [Advanced Workflows](./libvirt-advanced.md) - Complex deployment patterns
- [Storage Management](./storage-management.md) - Advanced storage concepts