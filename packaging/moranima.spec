# Packages the prebuilt release binary: rpmbuild -bb --define "_srcroot <repo>" moranima.spec
%define debug_package %{nil}

Name:           moranima
Version:        0.1.0
Release:        1
Summary:        Animate a still character image — blink, lip-sync, background FX
License:        Unlicense
URL:            https://github.com/MoribundMurdoch/MorAnima
Requires:       webkit2gtk4.1, gtk3
Recommends:     ffmpeg

%description
MorAnima layers animation effects on a single still character image:
liquify-style blinking, jaw-drop talking with real audio lip-sync,
background removal, animated backdrops and particle overlays. Exports
transparent WebM, MP4, GIF, or a Kdenlive-ready PNG frame sequence
through ffmpeg.

%install
install -Dm755 %{_srcroot}/target/release/moranima %{buildroot}%{_bindir}/moranima
install -Dm644 %{_srcroot}/packaging/moranima.desktop %{buildroot}%{_datadir}/applications/moranima.desktop
install -Dm644 %{_srcroot}/assets/icon.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/moranima.svg
for s in 16 24 32 48 64 128 256 512; do
  install -Dm644 %{_srcroot}/assets/icons/moranima-$s.png %{buildroot}%{_datadir}/icons/hicolor/${s}x${s}/apps/moranima.png
done
install -Dm644 %{_srcroot}/LICENSE %{buildroot}%{_datadir}/licenses/%{name}/LICENSE
install -Dm644 %{_srcroot}/README.md %{buildroot}%{_datadir}/doc/%{name}/README.md

%files
%{_bindir}/moranima
%{_datadir}/applications/moranima.desktop
%{_datadir}/icons/hicolor/*/apps/moranima.png
%{_datadir}/icons/hicolor/scalable/apps/moranima.svg
%{_datadir}/licenses/%{name}/LICENSE
%{_datadir}/doc/%{name}/README.md
