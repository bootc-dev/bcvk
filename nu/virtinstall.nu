#!/bin/nu

let arch = arch
# These are passed to virt-install --location
let anaconda_locations = [[name location];
    [fedora42 $"https://dl.fedoraproject.org/pub/fedora/linux/releases/42/Everything/($arch)/os/" ]
    [centos10 $"https://mirror.stream.centos.org/10-stream/BaseOS/($arch)/os/"]
]
let anaconda_aliases = [[name value];
    [fedora "fedora42"]
]

let kickstart_base = """
%pre --erroronfail
mkdir /mnt/host-container-storage
mount -t virtiofs -o ro host-container-storage /mnt/host-container-storage

%end
"""

# Inspect an image in the container storage
def inspect [image: string] nothing -> json {
    hostexec podman image inspect $image | from json | first
}

# Parse an os-release file into structured data
def parse_osrelease [] string -> record {
    split row "\n"
    | where { |it| $it != "" }
    | split column "=" key value
    | update value {|row|
        # Unquote if needed
        if (($row.value | str starts-with '"') and ($row.value | str ends-with '"')) {
            $row.value | str substring 1..($row.value | str length | $in - 1)
        } else {
            $row.value
        }
    }
    | reduce --fold {} { |it, acc| $acc | insert $it.key $it.value }
}

# Extract the /usr/lib/os-release from an iamge as structured data
def osrelease [image: string] {
    hostexec podman run --entrypoint cat --rm $image -c /usr/lib/os-release
        | parse_osrelease
}

# Wraps virt-install's --location support,
# targeting a bootc container image.
# export def "main libvirt install-anaconda" [
#     image: string        # container image to install
#     --name: string       # Name for the virtual machine
#     --os:  string        # operating system base; can be fedora (=fedora42) or centos10
#     --kickstart: path    # Path to kickstart
# ] {

#     let osrelease = 
#     let os = if ($os == null) {
#         if (($os_like | find centos) != null) {
#             "centos10"
#         } else {
#             "fedora"
#         }
#     } else {
#         $os
#     }
#     let resolved = match ($anaconda_aliases | where name == $os) {
#         [] => $os
#         [$o] => ($o | get value)
#     }
#     print $"Installing ($inspect.Id) \(osrelease ID=($osid) ID_LIKE=($os_like)\) using installation media for ($resolved)"
#     let location = $anaconda_locations | where name == $resolved
#     if ($location | length) == 0 {
#         error make { msg: $"Unknown OS: ($resolved)" }
#     }
#     let location = $location | first | get location
#     print "Running virt-install..."
#     let args = ["virt-install", 
#         "--noautoconsole",
#         $"--location=($location)",
#         --filesystem=/var/home/walters/.local/share/containers/storage/,host-container-storage,driver.type=virtiofs
#         --memorybacking=source.type=memfd,access.mode=shared
#     ]

#     ]        
#     print $"Running: ($args)"
#     hostexec ...$args
# }

# Boots a stock cloud image and uses system-reinstall-bootc
export def "main libvirt install-srb" [
    image: string        # container image to install
    --name: string       # Name for the virtual machine
    --os:  string        # operating system base; can be fedora (=fedora42) or centos10
] {
    let inspect = inspect $image
    let osrelease = osrelease $image
    let osid = $osrelease.ID?
    let os_like = $osrelease.ID_LIKE? | default ""
    let os = if ($os != null) {
        $os
    } else {
        if (($os_like | find centos) != null) {
            "centos10"
        } else {
            "fedora"
        }
    }
    let resolved = match ($anaconda_aliases | where name == $os) {
        [] => $os
        [$o] => ($o | get value)
    }
    print $"Installing ($inspect.Id) \(osrelease ID=($osid) ID_LIKE=($os_like)\) using installation media for ($resolved)"
    let location = $anaconda_locations | where name == $resolved
    if ($location | length) == 0 {
        error make { msg: $"Unknown OS: ($resolved)" }
    }
    let location = $location | first | get location
    print "Running virt-install..."
    let args = ["virt-install", 
        "--noautoconsole",
        $"--location=($location)",
        --filesystem=/var/home/walters/.local/share/containers/storage/,host-container-storage,driver.type=virtiofs
        --memorybacking=source.type=memfd,access.mode=shared
        
    ]

    ]        
    print $"Running: ($args)"
    hostexec ...$args
}

export def "main libvirt list" [] {

}
