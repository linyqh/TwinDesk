# 同桌 · TwinDesk 图标概念

使用内置 image_gen 生成，各方案为独立 PNG。已选定 **02 共享桌面** 并接入应用文件、主窗口、菜单栏和项目 README。

正式源图为 [TwinDesk.png](../../../apps/macos/Sources/OpenShareMouse/Resources/TwinDesk.png)，保留选定原图的构图与透明背景。打包脚本使用同一源图生成 16–1024 像素的 macOS 图标并封装为 `TwinDesk.icns`。

| 编号 | 方案 | 方向 |
|---|---|---|
| 01 | [并肩双屏](01-side-by-side.png) | 两台电脑共用底座，稳定、直观 |
| 02 | [共享桌面](02-shared-desk.png) | 两台电脑坐在同一张桌子上，最呼应“同桌”的名称 |
| 03 | [跨屏光标](03-cross-screen-cursor.png) | 指针跨越两块屏幕，最直观地表达键鼠共享 |
| 04 | [双 T 同桌](04-twin-t.png) | 两个交叠的 T 组成共享桌面，偏抽象品牌标志 |

01 保留初版构图，02–04 使用第二轮版本；未选方案保留供设计参考。

完整生成提示词见 [prompts.md](prompts.md)，精修提示词见 [refinements.md](refinements.md)。
