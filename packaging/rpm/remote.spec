# Remote: a native GNOME remote for Roku TVs (Rust, GTK4, libadwaita).
#
# Crate dependencies are vendored (Source1, made by make-srpm.sh with
# `cargo vendor --locked`), following the Fedora Rust guidelines for
# packages built outside the Fedora crate ecosystem (COPR): gtk4-rs 0.11 and
# libadwaita-rs 0.9 are pinned exactly and not all of them are packaged.
# The binary links the system libgtk-4 and libadwaita-1 dynamically.
#
# Not affiliated with Roku, Inc.; "Roku" is their trademark.

%global app_id xyz.scottnelson.Remote

Name:           remote
Version:        0.2.0
Release:        1%{?dist}
Summary:        A Roku remote for the GNOME desktop

SourceLicense:  MIT
# remote itself: MIT. Statically linked crates (see cargo-vendor.txt):
# MIT, MIT OR Apache-2.0, Unlicense OR MIT, (MIT OR Apache-2.0) AND
# Unicode-3.0, MPL-2.0 (option-ext)
License:        MIT AND (MIT OR Apache-2.0) AND (Unlicense OR MIT) AND Unicode-3.0 AND MPL-2.0
URL:            https://github.com/NelsonScott/remote
Source0:        %{name}-%{version}.tar.gz
Source1:        %{name}-%{version}-vendor.tar.xz

ExclusiveArch:  %{rust_arches}

BuildRequires:  cargo-rpm-macros >= 26
BuildRequires:  pkgconfig(gtk4) >= 4.18
BuildRequires:  pkgconfig(libadwaita-1) >= 1.8
BuildRequires:  desktop-file-utils
BuildRequires:  appstream

Requires:       hicolor-icon-theme

%description
A small native GNOME remote for Roku TVs. It finds the TV on the local
network (SSDP) and talks to it over the plain HTTP External Control
Protocol: D-pad, Back, Home, playback, volume with press-and-hold repeat,
typing into TV search boxes and launching channels. No account, no cloud.

%prep
%autosetup -n %{name}-%{version} -p1 -a1
%cargo_prep -v vendor

%build
%cargo_build
%{cargo_license_summary}
%{cargo_license} > LICENSE.dependencies
%{cargo_vendor_manifest}

%install
install -Dpm0755 target/rpm/remote %{buildroot}%{_bindir}/remote
install -Dpm0644 data/%{app_id}.desktop %{buildroot}%{_datadir}/applications/%{app_id}.desktop
for s in 16 24 32 48 64 128 256 512; do
    install -Dpm0644 data/icons/hicolor/${s}x${s}/apps/%{app_id}.png \
        %{buildroot}%{_datadir}/icons/hicolor/${s}x${s}/apps/%{app_id}.png
done
install -Dpm0644 data/%{app_id}-symbolic.svg %{buildroot}%{_datadir}/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg
install -Dpm0644 data/%{app_id}.metainfo.xml %{buildroot}%{_metainfodir}/%{app_id}.metainfo.xml

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/%{app_id}.desktop
appstreamcli validate --no-net %{buildroot}%{_metainfodir}/%{app_id}.metainfo.xml
%cargo_test

%files
%license LICENSE LICENSE.dependencies cargo-vendor.txt
%doc README.md
%{_bindir}/remote
%{_datadir}/applications/%{app_id}.desktop
%{_datadir}/icons/hicolor/16x16/apps/%{app_id}.png
%{_datadir}/icons/hicolor/24x24/apps/%{app_id}.png
%{_datadir}/icons/hicolor/32x32/apps/%{app_id}.png
%{_datadir}/icons/hicolor/48x48/apps/%{app_id}.png
%{_datadir}/icons/hicolor/64x64/apps/%{app_id}.png
%{_datadir}/icons/hicolor/128x128/apps/%{app_id}.png
%{_datadir}/icons/hicolor/256x256/apps/%{app_id}.png
%{_datadir}/icons/hicolor/512x512/apps/%{app_id}.png
%{_datadir}/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg
%{_metainfodir}/%{app_id}.metainfo.xml

%changelog
* Sun Nov 01 2026 Scott Nelson <8241108+NelsonScott@users.noreply.github.com> - 0.2.0-1
- Renamed to Remote (app id xyz.scottnelson.Remote, binary remote); first public release
- Old ~/.config/roku settings are copied over on first start

* Sat Oct 31 2026 Scott Nelson <8241108+NelsonScott@users.noreply.github.com> - 0.1.1-2
- New app icon (papercut remote), shipped as a hicolor PNG set (16 to 512 px)

* Sat Oct 31 2026 Scott Nelson <8241108+NelsonScott@users.noreply.github.com> - 0.1.1-1
- No false "Unreachable" while the TV is busy launching a channel
- Dark (Roam charcoal) by default, "Follow System Theme" menu option
- New app icon and a symbolic icon

* Sat Oct 31 2026 Scott Nelson <8241108+NelsonScott@users.noreply.github.com> - 0.1.0-1
- First package: remote UI, keyboard control, hold-to-repeat, channel grid,
  SSDP discovery with a unicast fallback for strict firewalls
