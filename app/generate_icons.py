#!/usr/bin/env python3
"""Generate all platform icons from a source PNG."""
from pathlib import Path
from PIL import Image

SCRIPT_DIR = Path(__file__).parent
SOURCE = Path(r"C:\Users\yzz\Desktop\handDir\Attachment\image_20260805_145850.png")
ICONS_DIR = SCRIPT_DIR / "src-tauri" / "icons"

def resize(img, size):
    return img.resize((size, size), Image.LANCZOS)

def save(img, path):
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(str(path), "PNG")
    print(f"  {path.relative_to(SCRIPT_DIR)}")

def main():
    src = Image.open(SOURCE).convert("RGBA")
    print(f"Source: {SOURCE} ({src.size[0]}x{src.size[1]})")
    print(f"Output: {ICONS_DIR}\n")

    # === PNG Icons ===
    print("=== PNG Icons ===")
    for name, size in {
        "32x32.png": 32,
        "64x64.png": 64,
        "128x128.png": 128,
        "128x128@2x.png": 256,
        "icon.png": 512,
    }.items():
        save(resize(src, size), ICONS_DIR / name)

    # === Windows Store Logos ===
    print("\n=== Windows Store Logos ===")
    for name, size in {
        "Square30x30Logo.png": 30,
        "Square44x44Logo.png": 44,
        "Square71x71Logo.png": 71,
        "Square89x89Logo.png": 89,
        "Square107x107Logo.png": 107,
        "Square142x142Logo.png": 142,
        "Square150x150Logo.png": 150,
        "Square284x284Logo.png": 284,
        "Square310x310Logo.png": 310,
        "StoreLogo.png": 50,
    }.items():
        save(resize(src, size), ICONS_DIR / name)

    # === ICO (Windows) ===
    print("\n=== ICO (Windows) ===")
    ico_sizes = [16, 24, 32, 48, 64, 128, 256]
    ico_images = [resize(src, s) for s in ico_sizes]
    ico_path = ICONS_DIR / "icon.ico"
    ico_images[0].save(
        str(ico_path), format="ICO",
        sizes=[(img.size[0], img.size[1]) for img in ico_images],
        append_images=ico_images[1:]
    )
    print(f"  {ico_path.relative_to(SCRIPT_DIR)}")

    # === iOS Icons ===
    print("\n=== iOS Icons ===")
    ios_dir = ICONS_DIR / "ios"
    for name, size in {
        "AppIcon-20x20@1x.png": 20,
        "AppIcon-20x20@2x.png": 40,
        "AppIcon-20x20@2x-1.png": 40,
        "AppIcon-20x20@3x.png": 60,
        "AppIcon-29x29@1x.png": 29,
        "AppIcon-29x29@2x.png": 58,
        "AppIcon-29x29@2x-1.png": 58,
        "AppIcon-29x29@3x.png": 87,
        "AppIcon-40x40@1x.png": 40,
        "AppIcon-40x40@2x.png": 80,
        "AppIcon-40x40@2x-1.png": 80,
        "AppIcon-40x40@3x.png": 120,
        "AppIcon-60x60@2x.png": 120,
        "AppIcon-60x60@3x.png": 180,
        "AppIcon-76x76@1x.png": 76,
        "AppIcon-76x76@2x.png": 152,
        "AppIcon-83.5x83.5@2x.png": 167,
        "AppIcon-512@2x.png": 1024,
    }.items():
        save(resize(src, size), ios_dir / name)

    # === Android Icons ===
    print("\n=== Android Icons ===")
    android_dir = ICONS_DIR / "android"
    for dpi, base in {"mdpi":48, "hdpi":72, "xhdpi":96, "xxhdpi":144, "xxxhdpi":192}.items():
        dpi_dir = android_dir / f"mipmap-{dpi}"
        fg = int(base * 512 / 48)
        for suffix in ["", "_round", "_foreground"]:
            sz = fg if "foreground" in suffix else base
            save(resize(src, sz), dpi_dir / f"ic_launcher{suffix}.png")

    print("\nDone!")

if __name__ == "__main__":
    main()
