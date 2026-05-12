{
  lib,
  rustPlatform,
  dbus,
  writeText,
}:
let
  portalFile = writeText "xdg-nvfilechooser.portal" ''
    [portal]
    DBusName=org.freedesktop.impl.portal.desktop.xdg-nvfilechooser
    Interfaces=org.freedesktop.impl.portal.FileChooser
  '';

  dbusService = writeText "org.freedesktop.impl.portal.desktop.xdg-nvfilechooser.service" ''
    [D-BUS Service]
    Name=org.freedesktop.impl.portal.desktop.xdg-nvfilechooser
    Exec=@out@/bin/xdg-nvfilechooser
    SystemdService=xdg-nvfilechooser.service
  '';

  systemdUnit = writeText "xdg-nvfilechooser.service" ''
    [Unit]
    Description=XDG Neovim filechooser backend
    After=graphical-session.target

    [Service]
    Type=dbus
    BusName=org.freedesktop.impl.portal.desktop.xdg-nvfilechooser
    ExecStart=@out@/bin/xdg-nvfilechooser
    Restart=on-failure

    [Install]
    WantedBy=graphical-session.target
  '';
in
rustPlatform.buildRustPackage {
  pname = "xdg-nvfilechooser";
  version = "0.1.0";
  src = ./.;
  cargoLock.lockFile = ./Cargo.lock;

  buildInputs = [ dbus ];

  postInstall = ''
    mkdir -p $out/share/xdg-desktop-portal/portals
    mkdir -p $out/share/dbus-1/services
    mkdir -p $out/lib/systemd/user
    cp ${portalFile} $out/share/xdg-desktop-portal/portals/xdg-nvfilechooser.portal
    substituteAll ${dbusService} $out/share/dbus-1/services/org.freedesktop.impl.portal.desktop.xdg-nvfilechooser.service
    substituteAll ${systemdUnit} $out/lib/systemd/user/xdg-nvfilechooser.service
  '';

  meta = with lib; {
    description = "Neovim-based filechooser backend for XDG Desktop Portal";
    platforms = platforms.linux;
    license = licenses.asl20;
  };
}
