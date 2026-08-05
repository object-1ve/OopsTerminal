#!/usr/bin/env python3
"""Generate all platform icons from SVG source using Pillow directly."""
import io
from pathlib import Path
from PIL import Image, ImageDraw

SCRIPT_DIR = Path(__file__).parent
ICONS_DIR = SCRIPT_DIR / "src-tauri" / "icons"

def create_icon(size):
    """Create the OopsTerminal icon at the given size using Pillow."""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    # Scale factor
    s = size / 1024.0

    # Background: rounded rectangle with gradient-like color
    # Use light yellow/cream color
    bg_color = (254, 243, 199)  # #fef3c7

    # Draw rounded rectangle background
    margin = int(40 * s)
    radius = int(160 * s)
    draw.rounded_rectangle(
        [margin, margin, size - margin, size - margin],
        radius=radius,
        fill=bg_color
    )

    # Title bar area (slightly darker)
    tb_color = (245, 158, 11, 76)  # #f59e0b with alpha
    tb_overlay = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    tb_draw = ImageDraw.Draw(tb_overlay)
    tb_height = int(220 * s)
    tb_draw.rounded_rectangle(
        [margin, margin, size - margin, margin + tb_height],
        radius=radius,
        fill=tb_color
    )
    # Fix bottom corners of title bar
    tb_draw.rectangle(
        [margin, margin + radius, size - margin, margin + tb_height],
        fill=tb_color
    )
    img = Image.alpha_composite(img, tb_overlay)
    draw = ImageDraw.Draw(img)

    # Title bar dots (red, yellow, green)
    dot_y = int(130 * s)
    dot_r = int(28 * s)
    dot_colors = [(239, 68, 68), (234, 179, 8), (34, 197, 94)]
    dot_xs = [int(160 * s), int(248 * s), int(336 * s)]
    for cx, color in zip(dot_xs, dot_colors):
        draw.ellipse(
            [cx - dot_r, dot_y - dot_r, cx + dot_r, dot_y + dot_r],
            fill=color
        )

    # Terminal prompt: > symbol
    accent_color = (245, 158, 11)  # #f59e0b
    # Draw > as two lines meeting at a point
    line_width = max(int(80 * s), 2)
    points = [
        (int(260 * s), int(420 * s)),
        (int(420 * s), int(560 * s)),
        (int(260 * s), int(700 * s)),
    ]
    draw.line(points, fill=accent_color, width=line_width, joint="curve")

    # Cursor block _
    cursor_x = int(480 * s)
    cursor_y = int(620 * s)
    cursor_w = int(260 * s)
    cursor_h = int(80 * s)
    cursor_r = int(16 * s)
    draw.rounded_rectangle(
        [cursor_x, cursor_y, cursor_x + cursor_w, cursor_y + cursor_h],
        radius=cursor_r,
        fill=accent_color
    )

    return img

def save_png(img, path):
    """Save PIL Image as PNG."""
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(str(path), "PNG")
    print(f"  Created {path.relative_to(SCRIPT_DIR)}")

def main():
    print(f"Output: {ICONS_DIR}")

    # Standard PNG sizes needed by Tauri
    print("=== PNG Icons ===")
    png_sizes = {
        "32x32.png": 32,
        "64x64.png": 64,
        "128x128.png": 128,
        "128x128@2x.png": 256,
        "icon.png": 512,
    }
    for name, size in png_sizes.items():
        img = create_icon(size)
        save_png(img, ICONS_DIR / name)

    # Windows Store / UWP logos
    print("\n=== Windows Store Logos ===")
    store_sizes = {
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
    }
    for name, size in store_sizes.items():
        img = create_icon(size)
        save_png(img, ICONS_DIR / name)

    # ICO file (Windows)
    print("\n=== ICO (Windows) ===")
    ico_sizes = [16, 24, 32, 48, 64, 128, 256]
    ico_images = [create_icon(s) for s in ico_sizes]
    ico_path = ICONS_DIR / "icon.ico"
    ico_path.parent.mkdir(parents=True, exist_ok=True)
    ico_images[0].save(
        str(ico_path), format="ICO",
        sizes=[(img.size[0], img.size[1]) for img in ico_images],
        append_images=ico_images[1:]
    )
    print(f"  Created {ico_path.relative_to(SCRIPT_DIR)}")

    # iOS icons
    print("\n=== iOS Icons ===")
    ios_dir = ICONS_DIR / "ios"
    ios_sizes = {
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
    }
    for name, size in ios_sizes.items():
        img = create_icon(size)
        save_png(img, ios_dir / name)

    # Android icons
    print("\n=== Android Icons ===")
    android_dir = ICONS_DIR / "android"
    android_dpis = {
        "mdpi": 48,
        "hdpi": 72,
        "xhdpi": 96,
        "xxhdpi": 144,
        "xxxhdpi": 192,
    }
    for dpi, base_size in android_dpis.items():
        dpi_dir = android_dir / f"mipmap-{dpi}"
        foreground_size = int(base_size * 512 / 48)
        for suffix in ["", "_round", "_foreground"]:
            size = foreground_size if "foreground" in suffix else base_size
            img = create_icon(size)
            save_png(img, dpi_dir / f"ic_launcher{suffix}.png")

    print("\nDone!")

if __name__ == "__main__":
    main()
