%global debug_package %{nil}
%global __os_install_post %{nil}
%global _build_id_links none
Name: tangledlink-desktop
Version: 0.2.0
Release: 1
Summary: TangledLink VPN client
License: LicenseRef-TangledLink
URL: https://tngl.pro
Source0: payload.tar
Requires: webkit2gtk4.1, gtk3, libayatana-appindicator-gtk3, polkit, iproute, nftables, libsecret, systemd-resolved

%description
TangledLink desktop client with a Polkit-authorized Xray TUN helper.

%prep
%build
%install
mkdir -p %{buildroot}
tar -xf %{SOURCE0} -C %{buildroot}

%files
%defattr(-,root,root,-)
/usr/bin/tangledlink-desktop
%dir /usr/lib/tangledlink-desktop
/usr/lib/tangledlink-desktop/tangledlink-tun-helper
/usr/lib/tangledlink-desktop/tangledlink-update-helper
/usr/lib/tangledlink-desktop/xray
/usr/lib/tangledlink-desktop/geoip.dat
/usr/lib/tangledlink-desktop/geosite.dat
/usr/share/applications/tangledlink-desktop.desktop
/usr/share/icons/hicolor/128x128/apps/tangledlink-desktop.png
/usr/share/polkit-1/actions/pro.tngl.desktop.policy
%license /usr/share/licenses/tangledlink-desktop/Xray-LICENSE
