FROM registry.redhat.io/ubi9/ubi:latest as build
RUN <<EORUN
set -xeuo pipefail
# Build dependencies
dnf -y install cargo rustc git-core
# And we'll inject nushell into the target, but in order to avoid
# depsolving twice, download it here.
dnf -y install https://dl.fedoraproject.org/pub/epel/epel-release-latest-9.noarch.rpm
mkdir /out-rpms
cd /out-rpms
dnf download nu
EORUN
COPY . /src
WORKDIR /src
# See https://www.reddit.com/r/rust/comments/126xeyx/exploring_the_problem_of_faster_cargo_docker/
# We aren't using the full recommendations there, just the simple bits.
RUN --mount=type=cache,target=/src/target \ 
    --mount=type=cache,target=/root \
    make && make install install-nushell-config DESTDIR=/out

FROM registry.redhat.io/ubi9/ubi:latest
# Install target dependencies we downloaded in the build phase.
RUN --mount=type=bind,from=build,target=/build rpm -ivh /build/out-rpms/*.rpm
COPY --from=build /out/ /
ENTRYPOINT ["bootckit"]
CMD ["shell"]

