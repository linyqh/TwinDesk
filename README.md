# OpenShareMouse

在同一局域网内的多台电脑之间共享键盘、鼠标、剪贴板和文件。功能目标对标 ShareMouse Professional，使用自有协议，不与原版互通。

产品规格、架构和验收基线见 [ShareMouse-Professional-Reconstruction.md](ShareMouse-Professional-Reconstruction.md)（下文简称“报告”）。

**状态：骨架阶段。** 目前只能构建、签名并启动一个菜单栏应用，显示权限状态。还不能共享键鼠。

## 架构

```
OpenShareMouse.app（菜单栏应用，单进程）
├─ Swift（apps/macos）：SwiftUI 界面，以及 AppKit 适配层（剪贴板、拖放、覆盖层）
│     ↕ UniFFI，仅控制面（crates/ffi）
└─ Rust 静态库
   ├─ osm-model           ID、能力、几何、布局档案（报告 §5）
   ├─ osm-core            控制租约状态机、拓扑、输入路由，sans-IO（§6）
   ├─ osm-protocol        OSM/1 protobuf schema 与封帧（§7.4）
   ├─ osm-transport       QUIC/TLS、发现（§7.1–7.3，待实现）
   ├─ osm-transfer        剪贴板 offer、文件传输、路径校验（§9–10）
   └─ osm-platform-macos  权限探针、event tap 捕获与注入（§17.2）
```

输入热路径（event tap 回调、本机屏蔽、事件注入）放在 Rust 里，不经过 FFI。理由见 [docs/adr/0001-input-hot-path-in-rust.md](docs/adr/0001-input-hot-path-in-rust.md)。

## 构建

需要 macOS 14 以上、Xcode 16 以上，以及 Rust stable（版本由 `rust-toolchain.toml` 指定）。

```sh
make app          # 构建 Rust 和 Swift，打包并签名 build/app/OpenShareMouse.app
make run          # 构建后启动
make test         # Rust 测试 + Swift 绑定测试
make lint         # rustfmt + clippy
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
