# Packaging

Splashhound releases ship three Linux-friendly artifacts (built by
`.github/workflows/release.yml` on every `v*` tag):

- `splashhound-windows-x64.zip` — portable `splashhound.exe`
- `splashhound-linux-x64.tar.gz` — portable binary (built on Ubuntu 24.04)
- `splashhound_<version>_amd64.deb` — Debian/Ubuntu package

## Debian / Ubuntu

Build locally after compiling the app:

```bash
cargo build --release --manifest-path src-tauri/Cargo.toml
bash packaging/linux/build_deb.sh 1.3.0
# -> dist/splashhound_1.3.0_amd64.deb
```

Install with:

```bash
sudo apt install ./dist/splashhound_1.3.0_amd64.deb
```

Requires WebKitGTK 4.1 (Ubuntu 24.04+ / Debian 13+) — that dependency is
declared in the package, so `apt` will refuse to install it on older releases.

## Arch Linux

The PKGBUILD in `packaging/arch` builds the app from the GitHub tag tarball:

```bash
cd packaging/arch
updpkgsums          # once the GitHub repository is public
makepkg -si
```

It installs `/usr/bin/splashhound`, a desktop entry and icons.

### Publishing to the AUR

The AUR hosts build scripts only, and the source URL must be publicly
accessible — so **make the GitHub repository public first**.

1. **Create an AUR account** at <https://aur.archlinux.org> and add an SSH key
   (My Account → SSH Public Key). Test it:
   ```bash
   ssh aur@aur.archlinux.org     # should print a welcome message
   ```
2. **Prepare the package** on an Arch machine:
   ```bash
   cd packaging/arch
   updpkgsums                                  # real sha256 for the tag tarball
   makepkg --printsrcinfo > .SRCINFO           # refresh metadata
   makepkg -si                                 # test the build and install
   ```
3. **Create the package repository**. Either use "Submit Package" in the AUR
   web interface, or push to the SSH remote for a name that does not exist yet:
   ```bash
   git clone ssh://aur@aur.archlinux.org/splashhound.git
   cd splashhound
   cp /path/to/Splashhound/packaging/arch/PKGBUILD .
   cp /path/to/Splashhound/packaging/arch/.SRCINFO .
   git add PKGBUILD .SRCINFO
   git commit -m "Initial import: splashhound 1.0.0"
   git push
   ```
4. **Update for a new release**:
   ```bash
   # bump pkgver in PKGBUILD, then
   updpkgsums
   makepkg --printsrcinfo > .SRCINFO
   git commit -am "upgpkg: splashhound 1.0.1-1"
   git push
   ```
   Also bump `pkgrel` when only packaging changes.

The AUR package name `splashhound` was free at the time of writing. If it gets
taken, rename the package (and the repository) — for example
`splashhound-bin` using the release tarball instead of the source archive.
