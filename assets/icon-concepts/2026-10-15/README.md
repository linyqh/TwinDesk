# 同桌 · TwinDesk 图标概念

使用内置 image_gen 生成，各方案为独立 PNG。已选定 **02 共享桌面** 并接入应用文件、主窗口、菜单栏和项目 README。

正式源图为 [TwinDesk.png](../../../apps/macos/Sources/OpenShareMouse/Resources/TwinDesk.png)，应用内仍使用该透明 PNG。系统图标使用 [Icon Composer 配置](../../../apps/macos/Support/TwinDesk.icon/icon.json)，由打包脚本临时导入同一源图，使用 Xcode 26+ 的 `actool` 编译为 `Assets.car` 和兼容旧系统的 `TwinDesk.icns`。两者均须放在应用的 `Contents/Resources`，并通过 `CFBundleIconName = TwinDesk` 引用原生图标。

原来的纯 ICNS 在新系统中会被缩小并套上灰色底板。原生图标将源图放大至 1.2 倍，使蓝色背景覆盖系统蒙版，关闭额外的玻璃和投影效果，保留双屏木桌的原始外观。不要退回只复制 ICNS 的打包方式。

| 编号 | 方案 | 方向 |
|---|---|---|
| 01 | [并肩双屏](01-side-by-side.png) | 两台电脑共用底座，稳定、直观 |
| 02 | [共享桌面](02-shared-desk.png) | 两台电脑坐在同一张桌子上，最呼应“同桌”的名称 |
| 03 | [跨屏光标](03-cross-screen-cursor.png) | 指针跨越两块屏幕，最直观地表达键鼠共享 |
| 04 | [双 T 同桌](04-twin-t.png) | 两个交叠的 T 组成共享桌面，偏抽象品牌标志 |

01 保留初版构图，02–04 使用第二轮版本；未选方案保留供设计参考。

完整生成提示词见 [prompts.md](prompts.md)，精修提示词见 [refinements.md](refinements.md)。
