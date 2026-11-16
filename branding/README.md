# Kitsunarr branding

<img src="kitsunarr-mark.svg" alt="Kitsunarr kitsune logo" width="112" height="112">

Kitsunarr's mark is a friendly kitsune with oversized ears, a cream muzzle, bright eyes,
rosy cheeks and a spirit-flame marking. The transparent SVG works on light and dark
backgrounds, including the application's sidebar and sign-in/setup screens.

| Asset | Purpose |
|---|---|
| [kitsunarr-mark.svg](kitsunarr-mark.svg) | Editable vector source; preferred for scalable icons |
| [kitsunarr-mark.png](kitsunarr-mark.png) | Transparent 512 × 512 mark |
| [kitsunarr-wordmark-light.svg](kitsunarr-wordmark-light.svg) | Wordmark for light backgrounds |
| [kitsunarr-wordmark-dark.svg](kitsunarr-wordmark-dark.svg) | Wordmark for dark backgrounds |
| [github-social-preview.png](github-social-preview.png) | 1280 × 640 repository social card, under 1 MB |
| [github-social-preview.svg](github-social-preview.svg) | Editable vector social card |

The README selects the wordmark for the viewer's color scheme. The app uses exports of the
same mark for its SVG/PNG/ICO favicons and Apple touch icon. The logo remains recognizable
at small sizes; use the mark alone below the wordmark's comfortable reading size.

## Colors

- Orange: `#f28c28`
- Cream: `#fff4e4`
- Ink: `#332333`
- Blush: `#ee9a9b`

## Updating the artwork

Edit `kitsunarr-mark.svg`, then run:

```sh
python scripts/export-branding.py
```

The exporter requires Python 3, Pillow, `rsvg-convert` (librsvg) and DejaVu Sans for text rendering. It derives all icon,
wordmark and social-card exports from the source mark. These are development tools; the
running application has no new dependencies. Source artwork is original to Kitsunarr and
uses the repository's GPL-3.0-or-later license.

## GitHub social preview

The repository README displays the new logo after the commit is pushed. GitHub's custom
repository social preview is a separate setting, with no upload operation in its public
repository API or `UpdateRepositoryInput` GraphQL mutation. To apply the prepared image,
open [repository Settings](https://github.com/KvasFan/Kitsunarr/settings), then **Social
preview → Edit → Upload an image**, and select `github-social-preview.png`.

GitHub's [social preview documentation](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/customizing-your-repositorys-social-media-preview)
recommends 1280 × 640 pixels and a file under 1 MB. A personal repository has its owner's
avatar; this artwork update does not change the owner's account identity.

Verification: [16 November 2026 branding checkpoint](../docs/branding-2026-11-16.md).
