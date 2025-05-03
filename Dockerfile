FROM registry.redhat.io/ubi9/ubi:latest as build
RUN <<EORUN
set -xeuo pipefail
dnf -y install cargo rustc
EORUN
COPY . /src
WORKDIR /src
# See https://www.reddit.com/r/rust/comments/126xeyx/exploring_the_problem_of_faster_cargo_docker/
# We aren't using the full recommendations there, just the simple bits.
RUN --mount=type=cache,target=/src/target \ 
    --mount=type=cache,target=/root \
    make && make install DESTDIR=/out 

FROM registry.redhat.io/ubi9/ubi:latest
COPY --from=build /out/ /
ENTRYPOINT ["/usr/bin/bootc-kit"]
