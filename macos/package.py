#!/usr/bin/env python3
"""Build a drag-to-Applications installer with Finder records for the mounted volume."""
from contextlib import contextmanager
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile

from ds_store import DSStore
from mac_alias import Alias, Bookmark

NAME = "剪栈"
WIDTH, HEIGHT = 768, 512
APP_POSITION = (395, 276)
APPLICATIONS_POSITION = (660, 276)


def run(*args, capture=False):
    return subprocess.run(args, check=True, stdout=subprocess.PIPE if capture else None).stdout


@contextmanager
def mounted(image, *options):
    entities = plistlib.loads(run("hdiutil", "attach", "-plist", "-nobrowse", "-noautoopen",
                                 "-owners", "off", *options, str(image), capture=True))["system-entities"]
    volume = next((item for item in entities if "mount-point" in item), None)
    try:
        if volume is None:
            raise RuntimeError("磁盘映像未能挂载。")
        yield Path(volume["mount-point"])
    finally:
        run("hdiutil", "detach", entities[0]["dev-entry"])


def draw_installer(original, mode_or_app, destination, working_directory):
    developer = Path(os.environ.get("DEVELOPER_DIR", "/Applications/Xcode.app/Contents/Developer"))
    swift = developer / "Toolchains/XcodeDefault.xctoolchain/usr/bin/swift"
    sdk = developer / "Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk"
    script = Path(__file__).resolve().with_name("preview-installer.swift")
    run(str(swift), "-sdk", str(sdk), "-module-cache-path", str(working_directory / "modules"),
        str(script), str(original), str(mode_or_app), str(destination))


def make_background(original, working_directory):
    composed = working_directory / "Artwork.png"
    draw_installer(original, "--background", composed, working_directory)
    standard = working_directory / "Installer.png"
    retina = working_directory / "Installer@2x.png"
    run("sips", "-z", str(HEIGHT), str(WIDTH), "-s", "dpiWidth", "72", "-s", "dpiHeight", "72",
        str(composed), "--out", str(standard))
    run("sips", "-s", "dpiWidth", "144", "-s", "dpiHeight", "144",
        str(composed), "--out", str(retina))
    output = working_directory / ".background.tiff"
    run("tiffutil", "-cathidpicheck", str(standard), str(retina), "-out", str(output))
    return output


def layout(volume, background):
    alias = Alias.for_file(str(background))
    bookmark = Bookmark.for_file(str(background))
    with DSStore.open(str(volume / ".DS_Store"), "w+") as store:
        root = store["."]
        root["vSrn"] = ("long", 1)
        root["icvl"] = ("type", "icnv")
        root["bwsp"] = {
            "WindowBounds": f"{{{{100, 110}}, {{{WIDTH}, {HEIGHT}}}}}",
            "ShowStatusBar": False, "ShowPathbar": False, "ShowSidebar": False,
            "ContainerShowSidebar": False, "ShowToolbar": False, "SidebarWidth": 180,
            "PreviewPaneVisibility": False, "ShowTabView": False,
        }
        root["icvp"] = {
            "viewOptionsVersion": 1, "backgroundType": 2,
            "backgroundColorRed": 1.0, "backgroundColorGreen": 1.0, "backgroundColorBlue": 1.0,
            "backgroundImageAlias": alias.to_bytes(),
            "iconSize": 96.0, "textSize": 13.0, "arrangeBy": "none",
            "gridOffsetX": 0.0, "gridOffsetY": 0.0, "gridSpacing": 100.0,
            "showItemInfo": False, "showIconPreview": False, "labelOnBottom": True,
            "scrollPositionX": 0.0, "scrollPositionY": 0.0,
        }
        root["pBBk"] = bookmark
        store[NAME + ".app"]["Iloc"] = APP_POSITION
        store["Applications"]["Iloc"] = APPLICATIONS_POSITION


def verify(volume, app):
    background = volume / ".background.tiff"
    if not background.is_file() or (volume / "Applications").readlink() != Path("/Applications"):
        raise RuntimeError("安装镜像缺少背景或 Applications 链接。")
    run("codesign", "--verify", "--strict", str(volume / app.name))
    with DSStore.open(str(volume / ".DS_Store"), "r") as store:
        root = store["."]
        if store[app.name]["Iloc"] != APP_POSITION or store["Applications"]["Iloc"] != APPLICATIONS_POSITION:
            raise RuntimeError("安装镜像的图标坐标不符。")
        expected = Alias.for_file(str(background))
        embedded = Alias.from_bytes(root["icvp"]["backgroundImageAlias"])
        if (root["icvp"]["backgroundType"] != 2
                or embedded.target.cnid != expected.target.cnid
                or embedded.volume.creation_date != expected.volume.creation_date
                or not root["pBBk"]):
            raise RuntimeError("Finder 背景引用与镜像不符。")


def main():
    output = Path(sys.argv[1]).resolve()
    app = output / (NAME + ".app")
    version = plistlib.loads((app / "Contents" / "Info.plist").read_bytes())["CFBundleShortVersionString"]
    volume_name = f"{NAME} {version}"
    result = output / f"{NAME}-{version}.dmg"
    original = Path(__file__).resolve().parent / "Assets" / "Installer.png"

    with tempfile.TemporaryDirectory(prefix="jianzhan-dmg-") as temp:
        temp = Path(temp)
        source = temp / "volume"
        source.mkdir()
        run("ditto", str(app), str(source / app.name))
        (source / "Applications").symlink_to("/Applications")
        background = make_background(original, temp)
        shutil.copyfile(background, source / background.name)

        # makehybrid marks the volume root as Finder's opening folder. The writable
        # copy keeps that metadata while giving bookmarks a real mounted filesystem.
        hybrid = temp / "layout.dmg"
        run("hdiutil", "makehybrid", "-hfs", "-hfs-volume-name", volume_name,
            "-hfs-openfolder", str(source), "-o", str(hybrid), str(source))
        writable = temp / "writable.dmg"
        run("hdiutil", "convert", str(hybrid), "-format", "UDRW", "-o", str(writable))
        size_mib = (hybrid.stat().st_size + (1 << 20) - 1) // (1 << 20)
        run("hdiutil", "resize", "-size", f"{size_mib + 8}m", str(writable))
        with mounted(writable, "-readwrite") as volume:
            # HFS layout creation can add Finder information to copied bundle files.
            run("xattr", "-cr", str(volume / app.name))
            run("codesign", "--verify", "--strict", str(volume / app.name))
            layout(volume, volume / background.name)

        compressed = temp / result.name
        run("hdiutil", "convert", str(writable), "-format", "UDZO", "-o", str(compressed))
        run("hdiutil", "verify", str(compressed))
        with mounted(compressed, "-readonly") as volume:
            verify(volume, app)
        preview = temp / "安装界面预览.png"
        draw_installer(original, app, preview, temp)
        shutil.copyfile(compressed, result)
        shutil.copyfile(preview, output / preview.name)
    print(result)


if __name__ == "__main__":
    main()
