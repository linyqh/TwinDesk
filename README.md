# OpenShareMouse

在同一局域网内的多台电脑之间共享键盘、鼠标、剪贴板和文件。功能目标对标 ShareMouse Professional，使用自有协议，不与原版互通。

产品规格、架构和验收基线见 [ShareMouse-Professional-Reconstruction.md](ShareMouse-Professional-Reconstruction.md)（下文简称“报告”）。

**状态：v1。** 两台 Mac 通过一根雷雳数据线共用一套键盘鼠标。

## v1 功能

- 检测本机雷雳端口：哪个端口接了设备、链路速率、对端是什么电脑、是否支持雷雳网络。
- 检测“雷雳网桥”（`bridge0`）是否启用、是否有地址。
- 两台 Mac 上都运行本应用后，通过雷雳网桥自动发现对方并连接。只走雷雳数据线，不经过 Wi-Fi。
- 把鼠标移出屏幕边缘（左或右，可设置）就会切到对方电脑，键盘随之跟过去。从对方屏幕的相对一侧移回来，就回到本机。
- 安全退出：
  - 按 ⌃⌘⎋（Control-Command-Esc）立即回到本机。
  - 拔掉数据线或对方超过 3 秒无响应时，自动回到本机，并松开对方电脑上被按住的键。
  - 被控制的电脑上有人点击、按键或滚动时，立即收回控制权。
- 主窗口显示数据线状态、端口信息、对方电脑信息、延迟和当前控制状态。

v1 不包含剪贴板、文件传输、多台电脑、Windows。**v1 的连接没有加密，也没有配对验证**：雷雳网桥上任何运行本应用的电脑都能连上来。只在你自己控制的两台电脑之间使用。

## 使用方法

1. 用雷雳 / USB4 数据线连接两台 Mac。
2. 两台电脑都打开“系统设置 > 网络”，确认“雷雳网桥”已启用（默认就是启用的，地址会自动分配为 169.254.x.x）。
3. 两台电脑都运行 OpenShareMouse，并在“系统设置 > 隐私与安全性 > 辅助功能”里允许它。授权后点窗口里的“重试”，或重新打开应用。
4. 窗口显示“已连接”后，在“对方电脑位于本机的”里选择左侧或右侧，两台电脑要选相反的方向。
5. 把鼠标推出那一侧的屏幕边缘即可。

## 架构

```
OpenShareMouse.app（菜单栏应用，单进程）
├─ Swift（apps/macos）：SwiftUI 界面，以及 AppKit 适配层（剪贴板、拖放、覆盖层）
│     ↕ UniFFI，仅控制面（crates/ffi）
└─ Rust 静态库
   ├─ osm-model           ID、能力、几何、布局档案（报告 §5）
   ├─ osm-agent           引擎：数据线监测、对方连接、输入路由（router.rs 是纯逻辑，可单测）
   ├─ osm-core            屏幕边缘几何、按键规则，sans-IO
   ├─ osm-protocol        OSM/1 protobuf schema、封帧、发现广播
   ├─ osm-transport       雷雳网桥上的 UDP 发现与 TCP 连接（绑定 bridge0）
   ├─ osm-transfer        文件传输路径校验（v1 未使用）
   └─ osm-platform-macos  雷雳检测、权限探针、event tap 捕获与注入、光标控制
```

输入热路径（event tap 回调、本机屏蔽、事件注入）放在 Rust 里，不经过 FFI。理由见 [docs/adr/0001-input-hot-path-in-rust.md](docs/adr/0001-input-hot-path-in-rust.md)。

## 构建

需要 macOS 14 以上、Xcode 16 以上，以及 Rust stable（版本由 `rust-toolchain.toml` 指定）。

```sh
make app          # 构建 Rust 和 Swift，打包并签名 build/app/OpenShareMouse.app
make run          # 构建后启动
make test         # Rust 测试 + Swift 绑定测试
make lint         # rustfmt + clippy
cargo run -p osm-agent --example status   # 在终端打印雷雳检测结果
```

`make app` 实际依次执行：

1. `scripts/build-rust.sh`：编译 `osm-ffi` 静态库，用 UniFFI 生成 Swift 绑定，打包成 `apps/macos/Frameworks/OSMCoreFFI.xcframework`。
2. `scripts/bundle-app.sh`：`swift build`，组装 `.app`，开启 Hardened Runtime 并签名。

修改 Rust 的导出接口后，要重新运行 `scripts/build-rust.sh`，Swift 端才能看到变化。

### 签名与系统权限

macOS 按代码签名记录“辅助功能”等授权。如果用 ad-hoc 签名，每次重新编译后授权都会失效。`bundle-app.sh` 会自动使用钥匙串里的第一张 `Developer ID Application` 证书，也可以用 `OSM_SIGN_IDENTITY` 指定。

运行时需要的授权：

| 授权 | 用途 |
|---|---|
| 辅助功能 | 主动 event tap：控制其他电脑时屏蔽本机输入 |
| 输入监控 | 只监听型 event tap |
| 发送事件（显示在“辅助功能”中） | 注入来自其他电脑的输入 |
| 本地网络 | 发现和连接其他设备（macOS 15 起） |

应用不使用 App Sandbox，不上架 Mac App Store，以 Developer ID 签名加公证的方式分发。

### 在 Xcode 中开发

用 Xcode 打开 `apps/macos/Package.swift` 即可编辑和调试。但需要授权的功能，必须用 `make run` 构建出的已签名 `.app` 来测试。

## 目录

```
crates/        Rust workspace
tools/         workspace 内锁定版本的 uniffi-bindgen-swift
schema/        OSM/1 wire schema（protobuf）
apps/macos/    SwiftPM 包：App、生成的绑定、Info.plist、entitlements
scripts/       构建与打包脚本
probes/        平台能力最小实验（P0）
docs/          ADR、协议合同、兼容矩阵
```

## 路线

按报告 §15.1 推进。当前处于 **P0（平台可行性）**，待验证的探针清单见 [probes/README.md](probes/README.md)。
