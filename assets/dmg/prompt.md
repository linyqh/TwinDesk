# DMG background

Generated with the built-in image_gen tool. The supplied installer screenshots are visual references only. Real application and Applications icons are supplied by Finder, not painted into the background.

```text
Use case: ui-mockup
Asset type: production macOS DMG Finder window background bitmap, NOT a screenshot or mockup. Generate a 1440 x 960 landscape image intended to display at 720 x 480 logical points.
Input images: Image 1 is a style/composition reference only (yovoice installer). Do not reproduce its brand. Image 2 shows the current TwinDesk installer for context only.
Primary request: Create an elegant, minimal installation instruction background for 同桌 · TwinDesk, inspired by the first reference's airy off-white layout, rounded icon wells, large heading, gentle accent arrow and restrained typography.
Composition: flat full-bleed rectangular canvas, no window chrome or traffic-light buttons, no outer rounded frame. Coordinates below use a 720 x 480 logical canvas (render at 2x). Top left text starts at x44 y40. Top right small wordmark TwinDesk in muted periwinkle at x580 y48. Two extremely subtle rounded white icon wells centered at (180,228) and (540,228), each 152 wide x156 high. Leave both wells COMPLETELY EMPTY: Finder will render the real draggable app and Applications folder icons plus labels there. No drawn app icon, folder, placeholder symbol, or caption inside either well. Between the wells at y220 draw a refined slim horizontal blue-periwinkle arrow pointing right, softly graduated, hinting at two linked screens with a few understated dots. Plenty of whitespace.
Color palette: very pale cool white/light lavender background like the reference, dark charcoal title, readable cool gray body text, restrained periwinkle accent compatible with TwinDesk's blue app icon.
Exact text, clean modern Chinese sans-serif with accurate glyphs:
At (44,40), bold heading 27 logical px: "安装同桌 · TwinDesk"
At (44,89), 14 logical px: "将左侧应用拖入 Applications，然后从「应用程序」打开。"
At top right: "TwinDesk"
Below icon wells, left aligned at x44 y350, 14 logical px: "启动后常驻菜单栏，不显示 Dock 图标。"
Next line at x44 y377, 13 logical px: "首次使用，请按提示开启辅助功能、输入监控及本地网络权限。"
Bottom left x44 y440, 11 logical px muted: "macOS 14+ · Apple Silicon"
Bottom right aligned to x676 y440, 11 logical px muted: "同桌 · TwinDesk 0.1.0"
Constraints: all listed text must be verbatim and legible, no additional text, no repeated labels. Cards must remain blank for actual Finder icons. This is the actual background asset to ship, not a picture of a computer or a window. Match the refined calm visual language of reference 1.
```

## 1.1.2 footer update

Edited with the built-in image_gen tool, using `background@2x.png` as the edit target. The final image replaces that asset and is converted to the two-resolution `background.tiff` used by Finder.

```text
Edit the provided production macOS DMG installer background image. Make exactly one targeted text change: the bottom-right footer currently reads '同桌 · TwinDesk 0.1.0'; change it to '同桌 · TwinDesk 1.1.2'. Preserve its font, font size, baseline, right alignment, gray color. Preserve ALL other pixels/content/layout as closely as possible: all Chinese text, English text, two empty rounded icon wells, arrow, background, whitespace. Do not add icons or change any other wording. Output same 1536 x 1024 landscape dimensions, opaque background.
```

## 1.1.3 footer update

Edited with the built-in image_gen tool, replacing background@2x.png and rebuilding background.tiff.

```text
Edit this production macOS DMG installer background. Change ONLY bottom-right footer version from '同桌 · TwinDesk 1.1.2' to '同桌 · TwinDesk 1.1.3'. Preserve all other text, exact layout, empty icon wells, arrow, colors, fonts and dimensions 1536 x 1024. Maintain same font size and right alignment in footer. No extra changes.
```

## 1.1.4 footer update

Edited with the built-in image_gen tool, replacing background@2x.png and rebuilding the two-resolution background.tiff.

```text
Use case: text-localization. Edit target: provided production macOS DMG installer background image. Change ONLY the bottom-right footer from '同桌 · TwinDesk 1.1.3' to '同桌 · TwinDesk 1.1.4'. Preserve all other text, exact layout, two empty icon wells, arrow, background, colors, fonts, whitespace and dimensions 1536 x 1024. Keep the footer font size, gray color, baseline and right alignment unchanged. No additional changes. Opaque background.
```
