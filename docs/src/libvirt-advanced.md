# Advanced libvirt Usage

## Multi-VM Deployments

You can run multiple VMs simultaneously, each with different container images or configurations.

Create isolated VMs for different application tiers (web servers, databases, etc.) using separate libvirt networks for network segmentation.

## Storage Configuration

Use libvirt storage pools to manage VM disk images. By default, bcvk creates VM disks in the default libvirt storage pool.

For shared storage across VMs, configure libvirt storage pools backed by NFS, iSCSI, or other network storage.

## Network Configuration

Create custom libvirt networks for VM isolation:
- Use libvirt's network XML definitions
- Configure NAT, routed, or isolated networks
- Set up DHCP ranges and static IP assignments

For direct host network access, use bridged networking or macvtap interfaces.

### Network Isolation

Use `--network-isolation` to block all outbound traffic from the VM while
preserving SSH access from the host:

```bash
bcvk libvirt run --name hermetic-test \
  --network-isolation \
  --bind-storage-ro \
  quay.io/centos-bootc/centos-bootc:stream10
```

This is particularly useful for CI/CD pipelines where test execution should
not depend on external network resources.  Pre-stage any required container
images or packages on the host and share them into the VM via `--bind`,
`--bind-ro`, or `--bind-storage-ro`.

## Automation with Scripts

Use shell scripts or configuration management tools to automate VM provisioning and management with bcvk libvirt commands.

VM definitions can be templated and version-controlled alongside your container images.

## See Also

- [bcvk-libvirt(8)](./man/bcvk-libvirt.md) - Command reference
- [Libvirt Integration](./libvirt-run.md) - Basic usage
- Libvirt documentation at libvirt.org for network and storage pool configuration