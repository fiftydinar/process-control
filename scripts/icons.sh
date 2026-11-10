#!/bin/sh
# Regenerates every raster icon from assets/icon.svg, or from assets/icon.png
# when the app's icon is pixel art.
#
# A PNG source is scaled with nearest-neighbour, so no new colors appear; a
# pixel of the art stays a square block at every size that is a whole
# multiple of the art's grid, and comes out uneven at any other. An SVG
# source is rendered at each size instead, which is what a vector icon wants.
#
# Outputs:
#   assets/icons/<size>.png           window icon, hicolor theme icons
#   docs/public/favicon.svg           assets/favicon.svg when present
#   docs/public/logo.svg              docs nav logo
#   docs/public/apple-touch-icon.png
#
# Needs resvg (cargo install resvg), and ImageMagick for a PNG source.
set -eu
cd "$(dirname "$0")/.."

if [ -f assets/icon.png ]; then
    source_png=assets/icon.png
    if command -v magick >/dev/null 2>&1; then
        magick=magick
    else
        magick=convert
    fi
    # -resize keeps the aspect ratio, so a non-square source would make
    # non-square icons without complaint.
    dims=$("$magick" "$source_png" -format '%w %h' info:)
    if [ "${dims% *}" != "${dims#* }" ]; then
        echo "assets/icon.png is ${dims% *}x${dims#* }; a pixel-art icon must be square" >&2
        exit 1
    fi
else
    source_png=
fi
svg=assets/icon.svg

# The source as an SVG, which is what resvg is given for the touch icon and
# what the docs use as favicon and logo. A pixel-art source is wrapped in
# one that carries the raster and asks browsers not to smooth it, kept
# outside the repository so a failed run leaves nothing behind.
wrapped=$(mktemp)
trap 'rm -f "$wrapped"' EXIT
if [ -n "$source_png" ]; then
    {
        printf '<svg xmlns="http://www.w3.org/2000/svg" '
        printf 'xmlns:xlink="http://www.w3.org/1999/xlink" '
        printf 'viewBox="0 0 1024 1024" width="1024" height="1024">\n'
        printf '  <image width="1024" height="1024" '
        printf 'style="image-rendering:pixelated" '
        printf 'xlink:href="data:image/png;base64,'
        base64 < "$source_png" | tr -d '\n'
        printf '"/>\n</svg>\n'
    } > "$wrapped"
    svg=$wrapped
fi

favicon=assets/favicon.svg
[ -f "$favicon" ] || favicon=$svg

mkdir -p assets/icons docs/public
for size in 16 24 32 48 64 128 256 512 1024; do
    if [ -n "$source_png" ]; then
        "$magick" "$source_png" -filter point -resize "${size}x${size}" \
            "assets/icons/$size.png"
    else
        resvg -w "$size" -h "$size" "$svg" "assets/icons/$size.png"
    fi
done

cp "$favicon" docs/public/favicon.svg
cp "$svg" docs/public/logo.svg
resvg --image-rendering pixelated -w 180 -h 180 "$svg" \
    docs/public/apple-touch-icon.png
echo "icons regenerated from ${source_png:-assets/icon.svg}"
