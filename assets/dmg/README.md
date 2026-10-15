# DMG 安装背景

使用内置 image_gen 参考用户提供的简洁安装窗口生成，完整提示词见 [prompt.md](prompt.md)。

- `background@2x.png`：1536 × 1024 原始生成图。
- `background.tiff`：768 × 512 与 1536 × 1024 双分辨率 TIFF，供 Finder 在普通及 Retina 屏幕显示。
- 应用和 Applications 是 Finder 提供的真实可拖拽图标；背景仅包含说明文字、空白底板和箭头。

安装内容已融入背景，不再额外放置 `安装说明.txt`。

重新制作已签名 DMG（默认复用 `build/dist/notarized` 中已公证应用）：

```sh
uv venv build/dmg-tools
uv pip install --python build/dmg-tools/bin/python ds-store==1.3.3 mac-alias==2.2.3
bash scripts/package-dmg.sh
```

可传入应用路径及输出路径。脚本要求应用已有有效公证票据，并保留原应用签名与票据。默认输出文件名从应用的版本号生成。背景包含版本 1.1.4；发布其他版本时需同步更新背景。
