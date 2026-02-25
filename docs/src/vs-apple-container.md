# bcvk and Apple container integration

Apple's [`container`](https://github.com/apple/container) is a Swift-based
tool that runs Linux containers as lightweight virtual machines on Apple Silicon
Macs using the macOS Virtualization framework. It is macOS-only (requires macOS
26+ and Apple Silicon) and targets standard OCI container images.

bcvk runs *bootable* container images as VMs using QEMU/libvirt on Linux.

Despite both tools using VMs, they use them differently. Apple's `container`
runs container processes inside lightweight VMs — the VM is an isolation
mechanism wrapping what is conceptually still a container. bcvk boots a
complete OS from a container image — the VM *is* the end product, not an
implementation detail.

The interesting integration opportunity is that Apple's tool creates ext4
filesystem images from OCI layers and caches them on disk. bcvk could read
those ext4 images directly, extract the kernel, and boot them as full VMs —
avoiding the `bootc install to-disk` step on macOS.

## Background

Apple's `container` converts OCI images into ext4 filesystem images using
`EXT4Unpacker` from the
[Containerization](https://github.com/apple/containerization) Swift package,
then attaches them to VMs as virtio-blk devices. A `SnapshotStore` caches the
ext4 images keyed by manifest digest. The VM boots a separate minimal kernel
and `vminitd` guest agent — the kernel is *not* from the container image.

bcvk's current flows (ephemeral run via VirtioFS, to-disk via `bootc install`)
require bootc images that contain a kernel, initramfs, and systemd. The kernel
is always extracted from the container image itself.

## How Apple's ext4 pipeline works

Apple's `EXT4Unpacker` (in the
[Containerization](https://github.com/apple/containerization) package) does
roughly the following:

1. Creates a sparse ext4 filesystem image file via `EXT4.Formatter(path,
   minDiskSize: N)`. The default minimum size is 512 GiB for regular
   containers (sparse, so actual disk usage is much smaller).

2. Iterates through the OCI image manifest's layers in order. For each layer,
   it calls `filesystem.unpack(source: layer.path, ...)`, which reads the
   layer tarball (gzip, zstd, or uncompressed) and writes its contents
   directly into the ext4 image. OCI whiteout files (`.wh.*` and
   `.wh..wh..opq`) are handled inline — whiteout entries delete files from
   previous layers.

3. The result is a flat ext4 image containing the fully merged container
   rootfs. No union filesystem or overlay is needed at runtime.

This ext4 image is then attached to a lightweight VM as a virtio-blk device.
Inside the VM, a minimal guest agent (`vminitd`) mounts it and runs container
processes within Linux cgroups and namespaces. Critically, the kernel and
vminitd are *not* from the container image — they're provided separately by the
`container` tool's own "init image."

## How bcvk's current flows work

bcvk has two main paths for getting from container image to running VM:

**Ephemeral run** (`bcvk ephemeral run`): The container image is pulled via
podman and mounted directly as the VM's root filesystem using VirtioFS (via
virtiofsd). The kernel and initramfs are extracted from *within* the container
image (from `/usr/lib/modules/<version>/` or `/boot/EFI/Linux/*.efi`). The VM
boots with `rootfstype=virtiofs root=rootfs` and systemd takes over as init.
This requires a *bootc* image — one that contains a kernel, initramfs, and
systemd.

**To-disk** (`bcvk to-disk`): An ephemeral VM is launched using the approach
above, and within it, `bootc install to-disk` runs to install the OS to an
attached virtio-blk disk. The output is a full disk image (with partition
table, bootloader, etc.) suitable for libvirt or QEMU.

Both flows fundamentally require bootc images. Standard OCI containers (e.g.
`docker.io/library/nginx`) lack a kernel, initramfs, and systemd, so bcvk
can't boot them.

## Reusing Apple's ext4 images directly

Since Apple's `container` tool already synthesizes ext4 rootfs images and
caches them on disk (see the "Apple's storage APIs" section below), bcvk
doesn't need to reimplement ext4 synthesis. On macOS, if the user has already
pulled an image with Apple's `container` tool, the ext4 is sitting at a
well-known path:
`~/Library/Application Support/com.apple.containerization/snapshots/<manifest-digest>/snapshot`.

To boot that ext4 as a VM, bcvk needs to:

1. **Locate the ext4 snapshot** — resolve the image reference to a
   platform-specific manifest digest, strip the `sha256:` prefix, and look
   for the file at the snapshot store path.

2. **Extract the kernel** — read the kernel and initramfs out of the ext4
   image without mounting it (see below).

3. **Boot via QEMU** — direct kernel boot (`-kernel`/`-initrd`) with the
   ext4 image attached as a virtio-blk device and the right kernel command
   line to mount it as root.

This avoids reimplementing Apple's `EXT4Unpacker` entirely. bcvk becomes a
consumer of Apple's snapshot store rather than a competing image pipeline.

## Kernel extraction from the ext4 image

Apple's `container` ships its own pre-built kernel separately from the
container image. bcvk takes a different approach: the kernel always comes from
the container image itself. This is a core design principle — bcvk boots the
image's own kernel so the VM matches what would run in production. There is no
"ship a separate kernel" option.

For bootc images accessed via VirtioFS, bcvk already extracts the kernel from
the mounted filesystem using `find_kernel()` in `crates/kit/src/kernel.rs`.
That function searches for UKIs in `/boot/EFI/Linux/*.efi` and
`/usr/lib/modules/<version>/*.efi`, and for traditional `vmlinuz` +
`initramfs.img` pairs in `/usr/lib/modules/<version>/`. It operates on a
`cap_std::fs::Dir`, which requires the filesystem to be mounted or otherwise
accessible as a directory tree.

When working with Apple's ext4 snapshots, the rootfs is an ext4 image file
rather than a mounted directory. The kernel needs to be extracted from that
ext4 image *before* the VM boots (since QEMU's `-kernel` flag needs the kernel
as a host file). This creates a chicken-and-egg problem: we need to read the
ext4 to get the kernel, but we don't want to mount the ext4 (that would
require root or fuse).

The solution is to use a userspace ext4 reader. The
[`ext4-view`](https://github.com/nicholasbishop/ext4-view-rs) crate provides
read-only access to ext4 filesystems from a file or byte buffer, without
mounting. It's pure Rust, no unsafe, `no_std` compatible, and its API follows
`std::fs` conventions (`read()`, `read_dir()`, `metadata()`, `exists()`).

The implementation would work roughly as follows:

1. After locating the ext4 snapshot from Apple's store, open it with
   `ext4_view::Ext4::load_from_path()`.

2. Run the same kernel search logic that `find_kernel()` uses, but against
   the `Ext4` filesystem API instead of `cap_std::fs::Dir`. The search paths
   are identical: `/boot/EFI/Linux/*.efi`, `/usr/lib/modules/<version>/*.efi`,
   `/usr/lib/modules/<version>/vmlinuz` + `initramfs.img`.

3. Extract the kernel (and initramfs if present) to a temporary file on the
   host using `Ext4::read()`, which returns the file contents as `Vec<u8>`.

4. Pass the extracted kernel to QEMU via `-kernel` (and `-initrd` if
   applicable), with the ext4 image as a virtio-blk device.

This approach is attractive because `ext4-view`'s API maps closely to
`cap_std::fs::Dir`. The kernel search logic could be refactored to be generic
over a filesystem trait — something like a `ReadDir + Read + Metadata`
abstraction — that both `Dir` and `Ext4` implement. Alternatively, a simpler
approach: a second `find_kernel_in_ext4()` function that duplicates the search
logic against the `Ext4` type. Given that the search logic is ~90 lines, a
small amount of duplication may be acceptable for a first pass, with
deduplication via a trait coming later.

The `ext4-view` crate is Apache-2.0/MIT dual-licensed (compatible with bcvk's
licensing), has no unsafe code, and is actively maintained. It handles the ext4
format details (block groups, extent trees, directory entries) that would be
tedious to implement from scratch.

## What would be different from default `apple/container`

Even though bcvk would read `apple/container`'s ext4 images, the boot model is
fundamentally different. `apple/container`'s `vminitd` is a purpose-built gRPC
agent that manages container processes using Linux cgroups and namespaces
*within* the VM — essentially a container runtime inside a VM. bcvk boots
systemd and runs the full OS using the image's own kernel. The container
image *is* the OS.

This means the images bcvk can boot from `apple/container`'s snapshot store are
limited to those that contain a kernel — bootc-style images. For images that
lack a kernel entirely (e.g. `docker.io/library/nginx`), bcvk would not
attempt to boot them. That's not bcvk's use case.

## Practical assessment

The implementation path for booting `apple/container`'s ext4 snapshots:

1. Locate the ext4 snapshot on disk. Resolve the image reference to a
   manifest digest (via the OCI index in `apple/container`'s content store
   or by querying `container` CLI) and find the file at
   `~/Library/Application Support/com.apple.containerization/snapshots/<digest>/snapshot`.

2. Use `ext4-view` to read the ext4 image and extract the kernel and
   initramfs to temporary host files, using the same search logic as the
   existing `find_kernel()`.

3. Boot via QEMU, macadam, or another VM tool with `-kernel`/`-initrd`
   pointing to the extracted files and the ext4 image as a virtio-blk root
   device.

4. Wire this into `bcvk ephemeral run` as a new path on macOS.

The hardest part is not reading the ext4 or extracting the kernel — both are
straightforward with `ext4-view`. The more interesting design question is
digest resolution: mapping an image reference to the right snapshot directory.

## The bootc-image-builder problem

The ephemeral boot flow described above works for `bcvk to-disk`: bcvk boots
the bootc image's ext4 as a VM, and inside that VM `bootc install to-disk`
writes to an attached virtio-blk disk. This is a self-contained operation —
the running VM has everything it needs.

[bootc-image-builder](https://github.com/osbuild/bootc-image-builder) (BIB)
is harder. BIB is itself a container that takes a *target* bootc image as
input and produces a disk image. The important distinction is that BIB is one
container image that needs to operate on a *different* container image.

Even on Linux, BIB integration with bcvk is not yet working. The WIP in
[PR #73](https://github.com/bootc-dev/bcvk/pull/73) (`osbuild-disk` command)
hits problems with deeply nested indirection: host -> podman -> bwrap -> qemu
-> podman (running BIB) -> osbuild + bubblewrap. The image fetching breaks
deep in the osbuild stack because of the layered container storage. The
emerging direction from that discussion is to use
[image-builder-cli](https://github.com/osbuild/image-builder-cli/) directly
inside the VM rather than running BIB as a container-within-a-VM, reducing
the nesting to just host -> qemu -> image-builder-cli -> osbuild.

On macOS with `apple/container`, the approach should mirror `bcvk to-disk`:
boot an ephemeral VM from the *target* bootc image, then run BIB inside that
VM. This is important because disk image creation (e.g. btrfs mkfs) needs the
target image's kernel and modules — you can't use an arbitrary kernel for this.

The flow would be:

1. bcvk boots an ephemeral VM from the target bootc image's ext4 (from
   `apple/container`'s snapshot store), using the target image's own kernel
   extracted via `ext4-view`. This is the same ephemeral boot flow described
   earlier.

2. The BIB container image's ext4 (also in `apple/container`'s snapshot
   store) is attached to the VM as an additional virtio-blk device.

3. Inside the VM — now running the target kernel — BIB runs from that second
   block device and operates on the target image's filesystem (which is the
   VM's own rootfs). An output disk is attached as a third virtio-blk device.

This requires `apple/container` to support passing additional block devices
into a VM beyond the single rootfs ext4 it attaches today. The `Filesystem`
type already supports multiple mount types (block, virtiofs, tmpfs) and the
VM configuration accepts a list of mounts keyed by ID, so the plumbing exists
in principle. The missing piece is a CLI or API surface to say "run this
container with this other image's ext4 attached as a second device."

This is a more involved integration than the ephemeral boot case. It depends
on both the Linux-side BIB nesting problem being solved (likely via
image-builder-cli) and an upstream contribution to `apple/container` for
additional block device passthrough.

## Apple's storage APIs and what they expose

The `Containerization` Swift package and the `container` tool's services expose
a layered set of APIs for accessing stored container images and their
synthesized ext4 filesystems. Understanding these APIs is useful for evaluating
whether bcvk (or any external tool) could reuse Apple's image storage directly.

### The content store: OCI blobs as files

The lowest layer is `LocalContentStore` (in `ContainerizationOCI`), which
implements a standard OCI content-addressable storage layout. Blobs are stored
as flat files at `<basePath>/blobs/sha256/<digest>`, where the default base
path is `~/Library/Application Support/com.apple.containerization/content/`.

The `ContentStore` protocol provides `get(digest:) -> Content?`, which returns
a `Content` object for any blob. The `Content` protocol exposes:

- `path: URL` — the filesystem path to the blob file
- `data() -> Data` — read the entire blob into memory
- `data(offset:length:) -> Data?` — read a range of the blob
- `size() -> UInt64` — file size
- `digest() -> SHA256.Digest` — content hash

`Image.getContent(digest:)` wraps this: given a digest that the image
references, it returns the `Content` object, from which you can get the `.path`
to the raw layer tarball on disk. The layers are stored as compressed tarballs
(gzip or zstd), exactly as pulled from the registry.

Any external tool that knows the digest of a layer can read it directly from
the filesystem without going through the Swift API — the layout is just files
in a well-known directory.

### The snapshot store: cached ext4 images

Above the content store sits `SnapshotStore` (in the `container` tool's
`ContainerImagesService`). This is where synthesized ext4 images are cached.
The layout on disk is `<basePath>/snapshots/<manifest-digest>/snapshot`, where
each `snapshot` file is a regular (sparse) ext4 filesystem image.

`SnapshotStore.get(for:platform:)` returns a `Filesystem` object describing
the cached ext4. The `Filesystem` type has a `source: String` field containing
the absolute path to the ext4 file, along with `type` (block, virtiofs, etc.),
`destination` (mount point), and `options` (mount options). For snapshots, the
type is `.block(format: "ext4", ...)` and the source points to the `snapshot`
file.

`SnapshotStore.unpack(image:platform:)` creates the ext4 if it doesn't already
exist: it delegates to `EXT4Unpacker.unpack()`, which iterates the image's
layers in order, unpacking each compressed tarball directly into an ext4 image
via `EXT4.Formatter`. The result is moved atomically into the snapshot
directory. Alongside the `snapshot` file, a `snapshot-info` JSON file stores
the serialized `Filesystem` metadata.

### Can an external tool read these files?

Yes, straightforwardly. Both the layer tarballs in the content store and the
ext4 images in the snapshot store are regular files. There is no database, no
proprietary container format, no locking mechanism that would prevent another
process from reading them. If Apple's `container` tool has already pulled an
image and unpacked it, bcvk could read the ext4 file directly from
`~/Library/Application Support/com.apple.containerization/snapshots/<digest>/snapshot`.

There are caveats. The snapshot store is keyed by the platform-specific
manifest digest (not the image reference or index digest), so you'd need to
resolve the image reference to the correct manifest digest to find the right
snapshot directory. The content store's digest-stripping convention
(`trimmingDigestPrefix` removes the `sha256:` prefix) is standard. Both stores
could be relocated if the user changes the base path.

### Could bcvk use the Swift APIs directly?

Not practically. The `Containerization` package is Swift-only and the ext4
writing code (`ContainerizationEXT4`) is gated behind `#if os(macOS)` — it
won't compile on Linux. The APIs are designed to be consumed from Swift
processes running on macOS.

However, bcvk doesn't *need* the APIs. Since the on-disk layout is simple and
well-defined, bcvk can read the ext4 snapshot files directly using their
filesystem paths. The `ext4-view` crate handles reading the ext4 contents
(for kernel extraction) without any dependency on Apple's Swift packages.

### Summary of the storage surface

The content store and snapshot store together form a clean two-tier cache:
compressed layer tarballs keyed by content digest, and materialized ext4 images
keyed by manifest digest. Both tiers are plain files on disk with predictable
paths. An external tool running on the same macOS system can read them without
any API dependency on Apple's Swift packages — all you need is the image digest
and knowledge of the directory layout.
