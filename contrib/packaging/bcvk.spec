%bcond_without check

Name:           bcvk
Version:        0.5.3
Release:        1%{?dist}
Summary:        Bootable container VM toolkit

# Apache-2.0 OR MIT
License:        Apache-2.0 OR MIT
URL:            https://github.com/bootc-dev/bcvk
Source0:        %{url}/releases/download/v%{version}/bcvk-%{version}.tar.zstd
Source1:        %{url}/releases/download/v%{version}/bcvk-%{version}-vendor.tar.zstd

# https://fedoraproject.org/wiki/Changes/EncourageI686LeafRemoval
ExcludeArch:    %{ix86}

BuildRequires: make
BuildRequires: openssl-devel
BuildRequires: go-md2man
BuildRequires: openssh-clients
%if 0%{?rhel}
BuildRequires: rust-toolset
%else
BuildRequires: cargo-rpm-macros >= 25
%endif

%description
%{summary}

%prep
%autosetup -p1 -a1
# Default -v vendor config doesn't support non-crates.io deps (i.e. git)
cp .cargo/vendor-config.toml .
%cargo_prep -N
cat vendor-config.toml >> .cargo/config.toml
rm vendor-config.toml

%build
%cargo_build

make manpages

%cargo_vendor_manifest
# https://pagure.io/fedora-rust/rust-packaging/issue/33
sed -i -e '/https:\/\//d' cargo-vendor.txt
%cargo_license_summary
%{cargo_license} > LICENSE.dependencies

%install
%make_install INSTALL="install -p -c"

%if %{with check}
%check
%cargo_test
%endif

%files
%license LICENSE-MIT
%license LICENSE-APACHE
%license LICENSE.dependencies
%license cargo-vendor.txt
%doc README.md
%{_bindir}/bcvk
%{_mandir}/man*/*bcvk*

%changelog
%autochangelog
