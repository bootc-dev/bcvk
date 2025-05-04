# List all bootc images in the container storage
export def "main images list" [] {
    let images = /usr/libexec/bootc-kit-backend hostexec podman images --filter label=containers.bootc=1 --format json | from json
    # Filter to key relevant data
    $images
        | select Names? Id Size CreatedAt
        | update Id { str substring 0..12 }
        | update Size { into filesize }
        | update CreatedAt { into datetime }
}

export def "main images" [] {
    help bootckit images
}
