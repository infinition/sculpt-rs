"""Package a native release on its target OS after cargo build --release."""
import argparse
import hashlib
import plistlib
import re
import shutil
import subprocess
import tarfile
import tempfile
import zipfile
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True, choices=[
        "x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu",
        "aarch64-apple-darwin", "x86_64-apple-darwin",
    ])
    parser.add_argument("--version", required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path("dist"))
    args = parser.parse_args()
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9.+_-]*", args.version):
        parser.error("version must be a filename-safe tag")
    if not args.binary.is_file():
        parser.error(f"binary missing: {args.binary}")
    repo = Path(__file__).resolve().parent.parent
    args.output.mkdir(parents=True, exist_ok=True)
    name = f"sculpt-rs-{args.version}-{args.target}"
    with tempfile.TemporaryDirectory(prefix="sculpt-package-") as temporary:
        folder = Path(temporary) / name
        folder.mkdir()
        for document in ["README.md", "LICENSE", "docs/release.md", "docs/workspace-and-brushes.md"]:
            source = repo / document
            if source.is_file():
                shutil.copy2(source, folder / source.name)
        if "apple" in args.target:
            app = folder / "Sculpt RS.app"
            contents = app / "Contents"
            executable = contents / "MacOS/sculpt-app"
            executable.parent.mkdir(parents=True)
            resources = contents / "Resources"
            resources.mkdir()
            shutil.copy2(args.binary, executable)
            executable.chmod(0o755)
            numeric = re.match(r"v?(\d+\.\d+\.\d+)", args.version)
            with (contents / "Info.plist").open("wb") as stream:
                plistlib.dump({
                    "CFBundleName": "Sculpt RS", "CFBundleDisplayName": "Sculpt RS",
                    "CFBundleIdentifier": "org.sculpt-rs.sculpt", "CFBundleExecutable": "sculpt-app",
                    "CFBundlePackageType": "APPL", "CFBundleShortVersionString": numeric[1] if numeric else "0.1.0",
                    "CFBundleVersion": numeric[1] if numeric else "0.1.0",
                    "CFBundleIconFile": "AppIcon", "NSHighResolutionCapable": True,
                    "LSMinimumSystemVersion": "11.0", "NSPrincipalClass": "NSApplication",
                }, stream)
            iconset = Path(temporary) / "AppIcon.iconset"
            iconset.mkdir()
            for size in [16, 32, 128, 256, 512]:
                for scale in [1, 2]:
                    suffix = "@2x" if scale == 2 else ""
                    subprocess.run(["sips", "-z", str(size * scale), str(size * scale),
                                    str(repo / "assets/icon.png"), "--out",
                                    str(iconset / f"icon_{size}x{size}{suffix}.png")], check=True, stdout=subprocess.DEVNULL)
            subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(resources / "AppIcon.icns")], check=True)
            subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(app)], check=True)
        elif "windows" in args.target:
            shutil.copy2(args.binary, folder / "sculpt-app.exe")
        else:
            binary = folder / "sculpt-app"
            shutil.copy2(args.binary, binary)
            binary.chmod(0o755)
            shutil.copy2(repo / "assets/icon.png", folder / "sculpt-rs.png")
            (folder / "sculpt-rs.desktop").write_text(
                "[Desktop Entry]\nType=Application\nName=Sculpt RS\nExec=sculpt-app\n"
                "Icon=sculpt-rs\nCategories=Graphics;3DGraphics;\nTerminal=false\n",
                encoding="utf-8", newline="\n")
        if "linux" in args.target:
            archive = args.output / f"{name}.tar.gz"
            with tarfile.open(archive, "w:gz") as output:
                output.add(folder, arcname=name)
        else:
            archive = args.output / f"{name}.zip"
            with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as output:
                for file in sorted(folder.rglob("*")):
                    if file.is_file():
                        output.write(file, file.relative_to(folder.parent))
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    # LF even when packaging on Windows: sha256sum --check reads the name
    # literally, and a trailing CR makes the archive unfindable on Linux.
    archive.with_name(archive.name + ".sha256").write_text(
        f"{digest}  {archive.name}\n", encoding="ascii", newline="\n")
    print(archive.resolve())


if __name__ == "__main__":
    main()
