# Grub Mascot

## Generation Parameters

| Parameter | Value |
|-----------|-------|
| Tool | mflux-generate |
| Model | `dhairyashil/FLUX.1-schnell-mflux-4bit` |
| Base model | `schnell` |
| Steps | `4` |
| Seed | `500` |
| Width | `1024` |
| Height | `1024` |

## Prompt

```
Adorable cartoon grub larva mascot character, happy expression, soft pastel green body, sitting on a leaf, modern app icon style, simple geometric shapes, flat vector art, cream beige background, hex F0E8DB
```

Background changed from white to cream beige (`#F0E8DB`) to match the grub.tools website canvas color. The generated background is approximately `#F9EBD2`, which blends well with the site.

## Regenerate

```sh
mflux-generate \
  --model dhairyashil/FLUX.1-schnell-mflux-4bit \
  --base-model schnell \
  --steps 4 \
  --seed 500 \
  --width 1024 \
  --height 1024 \
  --prompt "Adorable cartoon grub larva mascot character, happy expression, soft pastel green body, sitting on a leaf, modern app icon style, simple geometric shapes, flat vector art, cream beige background, hex F0E8DB" \
  --output assets/mascot/grub-mascot.png
```

**Important:** Only run one mflux-generate at a time — parallel runs will overwhelm the GPU.

## Web Variants

Generated from `grub-mascot.png` via Python/Pillow (see task instructions):

| File | Size | Format | Purpose |
|------|------|--------|---------|
| `website/assets/mascot.webp` | 512x512 | WebP (q85) | Primary web display |
| `website/assets/mascot.png` | 512x512 | PNG | Fallback |
| `website/assets/apple-touch-icon.png` | 180x180 | PNG | iOS home screen |
| `website/assets/favicon.ico` | 32x32 | ICO | Browser tab icon |
| `website/assets/og-image.png` | 1200x630 | PNG | Social sharing (Open Graph) |
