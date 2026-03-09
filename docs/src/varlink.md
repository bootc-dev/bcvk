# Using bcvk via varlink

bcvk exposes a [varlink](https://varlink.org/) interface for programmatic access.
This is useful for building tooling on top of bcvk without parsing CLI output.

At the current time there are varlink APIs for:

- `bcvk images` -- list bootc image names
- `bcvk ephemeral` -- launch and query ephemeral VMs, get SSH connection info
- `bcvk to-disk` -- create bootable disk images

The API is intentionally minimal: it exposes only operations that require
bcvk-specific knowledge. For example, `bcvk ephemeral rm-all` is not exposed
via varlink because you can do that directly via podman APIs.

## Via subprocess

bcvk serves varlink via [socket activation](https://varlink.org/#activation).
The idea is your higher level tool runs it as a subprocess, passing
the socket FD to it.

An example of this with `varlinkctl exec:`:

```bash
varlinkctl call exec:bcvk io.bootc.vk.images.List
```

## Introspecting

The varlink API is defined in the source code; to see the version of
the API exposed by the tool, use `varlinkctl introspect`:

```bash
varlinkctl introspect exec:bcvk io.bootc.vk.images
varlinkctl introspect exec:bcvk io.bootc.vk.ephemeral
varlinkctl introspect exec:bcvk io.bootc.vk.todisk
```

## SSH access to ephemeral VMs

After launching a VM with `Run(ssh_keygen: true)`, use `GetSshConnectionInfo`
to get the connection details:

```bash
varlinkctl call exec:bcvk io.bootc.vk.ephemeral.GetSshConnectionInfo \
    '{"container_id": "a1b2c3d4..."}'
```

This returns the container ID, key path, user, host, and port needed
to construct a `podman exec ... ssh ...` command:

```bash
podman exec <container_id> ssh -i <key_path> -p <port> \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    <user>@<host> [command...]
```
