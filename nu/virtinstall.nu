#!/bin/nu

# Wraps virt-install, targeting a bootc container image
export def "main libvirt install" [
    image: string # container image to install
    --os:  string
] {
    let inspect = /usr/libexec/bootc-kit-backend hostexec podman image inspect $image | from json | first
    let osrelease = /usr/libexec/bootc-kit-backend hostexec podman run --entrypoint bash --rm $image -c '. /usr/lib/os-release && echo $ID && echo $ID_LIKE'
        | split row "\n"
    
    print $"Installing ($image.Id) using ($os)"
}

export def "main libvirt list" [] {

}
