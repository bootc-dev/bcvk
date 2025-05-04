#!/bin/nu

let arch = arch
# These are passed to virt-install --location
let anaconda_locations = {
    fedora42: $"https://dl.fedoraproject.org/pub/fedora/linux/releases/42/Everything/($arch)/os/",
    centos10: $"https://mirror.stream.centos.org/10-stream/BaseOS/($arch)/os/",
}
let anaconda_aliases = {
    fedora: "fedora42"
}

# Wraps virt-install's --location support,
# targeting a bootc container image.
export def "main libvirt install" [
    image: string        # container image to install
    --name: string       # Name for the virtual machine
    --os:  string        # operating system base; can be fedora (=fedora42) or centos10
    --kickstart: path    # Path to kickstart
] {
    let inspect = hostexec podman image inspect $image | from json | first
    let osrelease = hostexec podman run --entrypoint bash --rm $image -c '. /usr/lib/os-release && echo $ID && echo $ID_LIKE'
        | split row "\n"
    let osid = $osrelease | first
    let os_like = $osrelease | last | default ""
    print $"os=($os) osid=($osid) os_like=($os_like)"
    let os = if ($os == null) {
        if (($os_like | find centos) != null) {
            print "using centos"
            "centos10"
        } else {
            print "using fedora"
            "fedora"
        }
    }
    let resolved = $anaconda_aliases | get $"($os)?"
    if ($resolved != null) {
        $resolved
    } else {
        $os
    }
    print $"Installing ($inspect.Id) \(osrelease ID=($osid) ID_LIKE=($os_like)\) using os=($resolved)"
    let location = $anaconda_locations | get $"($os)?"
    if (location == null) {
        error make { msg: "Unknown OS: $os" }
    }
    hostexec virt-install --noautoconsole $"--location=$location" --help
}

export def "main libvirt list" [] {

}
